use crate::{context, expand_home, git, index, pages, scan, search, semantic};
use anyhow::{Result, bail};
use regex::Regex;
use rusqlite::Connection;
use std::{
    collections::HashSet,
    env, fs,
    path::{Component, Path, PathBuf},
};

fn resolve(path: &Path) -> Result<PathBuf> {
    // Resolve existing prefixes too: deleted files still need path-scoped commits,
    // while a symlink must not turn an apparently local path into an outside one.
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => {
                out.push(other.as_os_str());
                match fs::symlink_metadata(&out) {
                    Ok(_) => out = fs::canonicalize(&out)?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    Ok(out)
}

pub fn wiki_paths(root: &Path, paths: &[String]) -> Result<Vec<String>> {
    let root_abs = fs::canonicalize(root)?;
    paths
        .iter()
        .map(|p| {
            // Accept the `type/slug` form that links and `links` output use, so a
            // path copied from there works for sync and check as well.
            let p = pages::link_path(p).unwrap_or_else(|| p.to_owned());
            let path = expand_home(Path::new(&p));
            let path = resolve(&if path.is_absolute() {
                path
            } else {
                root_abs.join(path)
            })?;
            let Ok(rel) = path.strip_prefix(&root_abs) else {
                bail!("{p} is not inside the wiki ({})", root.display());
            };
            Ok(if rel.as_os_str().is_empty() {
                ".".into()
            } else {
                rel.to_string_lossy().into_owned()
            })
        })
        .collect()
}

fn validate(root: &Path, rel: &str, text: &str, page: &pages::Page) -> i32 {
    let mut code = 0;
    if pages::page_type(rel).is_empty() {
        eprintln!(
            "warning: {rel} is not directly under pages/<{}>/",
            pages::TYPES.join("|")
        );
        code = code.max(1);
    }
    for problem in &page.problems {
        eprintln!("error: {rel} {}", scan::redact(problem));
        code = 2;
    }
    for finding in broken_links(root, page) {
        eprintln!("warning: {rel}: {finding}");
        code = code.max(1);
    }
    let budget = context::overview_budget();
    let length = page.body.chars().count();
    if page.kind == "projects" && length > budget {
        eprintln!(
            "warning: {rel}: body is {length} chars; only the first {budget} are shown at session start"
        );
        code = code.max(1);
    }
    for finding in scan::scan(text) {
        eprintln!("blocked: {rel}: {}: {}", finding.category, finding.excerpt);
        code = 2;
    }
    code
}

pub fn broken_links(root: &Path, page: &pages::Page) -> Vec<String> {
    page.links
        .iter()
        .filter(|target| !root.join(target).is_file())
        .map(|target| format!("link to {target}, which does not exist"))
        .collect()
}

fn shrink(old: &str, new: &str) -> Option<(usize, usize)> {
    let before = old.chars().count();
    let after = new.chars().count();
    (before.saturating_sub(after) > 1500 && (after as f64) < before as f64 * 0.6)
        .then_some((before, after))
}

fn duplicate_note(root: &Path, db: &Connection, rel: &str, text: &str) -> Result<()> {
    use rusqlite::OptionalExtension;
    let minimum = env::var("CLAUDE_WIKI_DUPLICATE_MIN")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && (-1.0..=1.0).contains(v))
        .unwrap_or(0.85);
    let mut candidates = vec![];
    for hit in semantic::similar(text, rel, minimum) {
        let Some(path) = pages::link_path(&hit.path) else {
            continue;
        };
        if path == rel || !hit.score.is_finite() || hit.score < minimum {
            continue;
        }
        let title: Option<String> = db.query_row("select fts.title from fts join files on files.id=fts.rowid where fts.path=? and files.blocked=''", [&path], |r| r.get(0)).optional()?;
        if let Some(title) = title {
            candidates.push(format!(
                "{} ({:.2}, {})",
                root.canonicalize()?.join(path).display(),
                hit.score,
                title.chars().take(120).collect::<String>()
            ));
        }
    }
    if !candidates.is_empty() {
        eprintln!(
            "note: {rel}: similar existing pages: {}. If one covers the same topic, update it instead and delete this page.",
            candidates.join("; ")
        );
    }
    Ok(())
}

fn written_page(
    root: &Path,
    db: &Connection,
    rel: &str,
    text: &str,
    page: &pages::Page,
) -> Result<i32> {
    if !matches!(page.kind.as_str(), "knowledge" | "followups") {
        return Ok(0);
    }
    match git(root, &["show", &format!("HEAD:{rel}")]) {
        Ok(old) => {
            if let Some((before, after)) = shrink(&old, text) {
                eprintln!(
                    "warning: {rel}: shrank from {before} to {after} chars; check that nothing was lost in a rewrite"
                );
                return Ok(1);
            }
        }
        Err(_) => duplicate_note(root, db, rel, text)?,
    }
    Ok(0)
}

#[derive(Default)]
struct Probe {
    pages: usize,
    queries: usize,
    failures: usize,
}

impl Probe {
    fn page(&mut self, db: &Connection, rel: &str, page: &pages::Page, top: i64) -> Result<()> {
        if page.queries.is_empty() {
            return Ok(());
        }
        self.pages += 1;
        for query in &page.queries {
            self.queries += 1;
            let (hits, dense_status) = search::ranked(
                db,
                &search::fts_query(std::slice::from_ref(query)),
                query,
                search::CANDIDATES,
                &search::Filters::default(),
                false,
            )?;
            let found = hits.iter().enumerate().find(|(_, hit)| hit.row.path == rel);
            let fused_rank = found.map(|(i, _)| i + 1);
            let bm25_rank = found.and_then(|(_, hit)| hit.bm25_rank);
            let dense_rank = found.and_then(|(_, hit)| hit.dense_rank);
            if [fused_rank, bm25_rank, dense_rank]
                .iter()
                .flatten()
                .any(|rank| *rank as i64 <= top)
            {
                continue;
            }
            self.failures += 1;
            let rank = |rank: Option<usize>, absent: &str| {
                rank.map_or_else(|| absent.to_owned(), |rank| format!("rank {rank}"))
            };
            let fused = rank(fused_rank, "absent");
            let bm25 = rank(bm25_rank, "not in top 40");
            let dense = match dense_status {
                semantic::DenseStatus::Disabled => "disabled".into(),
                semantic::DenseStatus::Unavailable(reason) => format!("unavailable ({reason})"),
                semantic::DenseStatus::Ok => rank(
                    dense_rank,
                    if hits.iter().any(|hit| hit.dense_rank.is_some()) {
                        "not in top 40"
                    } else {
                        "no results"
                    },
                ),
            };
            // Debug string quoting keeps multiline questions on one warning line.
            eprintln!(
                "warning: {rel}: query {:?} does not find this page (fused: {fused}, bm25: {bm25}, dense: {dense})",
                scan::redact(query)
            );
        }
        Ok(())
    }

    fn finish(&self) -> i32 {
        // A sync of pages without queries has nothing to report.
        if self.pages == 0 {
            return 0;
        }
        eprintln!(
            "probe: {} pages probed, {} queries, {} failures",
            self.pages, self.queries, self.failures
        );
        i32::from(self.failures > 0)
    }
}

pub fn check(
    root: &Path,
    db: &Connection,
    paths: &[String],
    probe: bool,
    probe_top: i64,
) -> Result<i32> {
    let paths = if paths.is_empty() {
        vec!["pages".into()]
    } else {
        wiki_paths(root, paths)?
    };
    let mut selected = vec![];
    for rel in paths {
        let path = root.join(rel);
        if path.is_dir() {
            selected.extend(pages::markdown_files(&path)?);
        } else if path.exists() {
            selected.push(path);
        }
    }
    selected.sort();
    selected.dedup();
    // Check needs existence only, not a separate git process and blob per page.
    let head = git(
        root,
        &["ls-tree", "-r", "--name-only", "-z", "HEAD", "--", "pages"],
    )
    .unwrap_or_default();
    let in_head: HashSet<_> = head.split('\0').filter(|path| !path.is_empty()).collect();
    let mut probes = Probe::default();
    let mut code = 0;
    for path in selected {
        let rel = path.strip_prefix(root)?.to_string_lossy();
        let text = pages::read(&path)?;
        let page = pages::parse(&text, &rel);
        let validation = validate(root, &rel, &text, &page);
        code = code.max(validation);
        if validation != 2 {
            if matches!(page.kind.as_str(), "knowledge" | "followups")
                && !in_head.contains(rel.as_ref())
            {
                duplicate_note(root, db, &rel, &text)?;
            }
            if probe {
                probes.page(db, &rel, &page, probe_top)?;
            }
        }
    }
    if probe {
        code = code.max(probes.finish());
    }
    Ok(code)
}

pub fn sync(root: &Path, db: &Connection, paths: &[String], message: Option<&str>) -> Result<i32> {
    let rels = wiki_paths(root, paths)?;
    let pathspec: Vec<&str> = if paths.is_empty() {
        vec![]
    } else {
        std::iter::once("--")
            .chain(rels.iter().map(String::as_str))
            .collect()
    };
    let run = |args: &[&str]| {
        let mut args = args.to_vec();
        args.extend(&pathspec);
        git(root, &args)
    };
    run(&["add", "-A"])?;
    let changes = run(&["diff", "--cached", "--name-status", "-z"])?;
    let fields: Vec<_> = changes.split('\0').filter(|s| !s.is_empty()).collect();
    let mut changes = vec![];
    let mut removed = vec![];
    let mut i = 0;
    while i < fields.len() {
        let status = fields[i];
        let count = if status.starts_with(['R', 'C']) { 2 } else { 1 };
        if status.starts_with(['D', 'R']) {
            removed.push(fields[i + 1]);
        }
        changes.push((status, fields[i + count]));
        i += count + 1;
    }
    if changes.is_empty() {
        println!("nothing to commit");
        return Ok(0);
    }
    let mut code = 0;
    let skills = installed_skills();
    let mut probes = Probe::default();
    for (status, rel) in &changes {
        if status.starts_with('D') || !rel.ends_with(".md") {
            continue;
        }
        // Validate exactly the staged content that a commit without paths uses.
        let text = git(root, &["show", &format!(":{rel}")])?;
        let page = pages::parse(&text, rel);
        let validation = validate(root, rel, &text, &page);
        code = code.max(validation);
        // Only when a page is written: a mention can be legitimate (a page citing a
        // skill as an example), and in `check` it would then warn forever.
        for name in untagged_skills(&page, &skills) {
            eprintln!(
                "warning: {rel}: mentions the {name} skill but is not tagged {name}; recall offers pages to a skill call by tag"
            );
            code = code.max(1);
        }
        if validation != 2 {
            code = code.max(written_page(root, db, rel, &text, &page)?);
            if page.kind == "knowledge" && page.queries.is_empty() {
                eprintln!(
                    "warning: {rel}: no queries; add the questions a future session would ask to find this page"
                );
                code = code.max(1);
            }
            probes.page(db, rel, &page, 3)?;
        }
    }
    code = code.max(probes.finish());
    if code == 2 {
        return Ok(2);
    }
    for rel in removed {
        let sources = index::linked_from(db, rel)?;
        if !sources.is_empty() {
            eprintln!(
                "warning: {rel} is linked from {} pages: {}",
                sources.len(),
                sources.join(", ")
            );
        }
    }
    let summary: Vec<_> = changes
        .iter()
        .map(|(status, rel)| {
            let verb = match status.chars().next().unwrap_or(' ') {
                'A' => "add",
                'M' => "update",
                'D' => "delete",
                'R' => "rename",
                'C' => "copy",
                _ => status,
            };
            let rel = rel.strip_prefix("pages/").unwrap_or(rel);
            format!("{verb} {}", rel.strip_suffix(".md").unwrap_or(rel))
        })
        .collect();
    let default = format!(
        "wiki: {}{}",
        summary
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        if summary.len() > 5 {
            format!(" (+{} more)", summary.len() - 5)
        } else {
            String::new()
        }
    );
    let message = message.filter(|s| !s.is_empty()).unwrap_or(&default);
    run(&["commit", "-q", "-m", message, "-m", &summary.join("\n")])?;
    println!("committed: {message}");
    if !pathspec.is_empty() {
        let others = git(root, &["status", "--porcelain"])?;
        let others: Vec<_> = others.lines().filter_map(|line| line.get(3..)).collect();
        if !others.is_empty() {
            println!(
                "left uncommitted (other sessions' changes): {}",
                others.join(", ")
            );
        }
    }
    Ok(code)
}

fn directories(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir())
        .collect()
}

fn default_dirs(home: &Path, config: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![config.join("skills")];
    let mut plugins = vec![home.join(".claude/plugins/cache")];
    for _ in 0..3 {
        plugins = plugins.iter().flat_map(|dir| directories(dir)).collect();
    }
    dirs.extend(plugins.into_iter().map(|dir| dir.join("skills")));
    dirs
}

fn installed_in(dirs: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<_> = dirs
        .iter()
        .flat_map(|dir| directories(dir))
        .filter(|path| path.join("SKILL.md").is_file())
        .filter_map(|path| path.file_name()?.to_str().map(str::to_lowercase))
        .filter(|name| name != "claude-wiki")
        .collect();
    names.sort();
    names.dedup();
    names
}

fn installed_skills() -> Vec<String> {
    let dirs = match env::var_os("CLAUDE_WIKI_SKILL_DIRS") {
        Some(value) => env::split_paths(&value)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| crate::expand_home(&path))
            .collect(),
        None => {
            let home = crate::home();
            let config = crate::env_path("CLAUDE_CONFIG_DIR")
                .map(|path| crate::expand_home(&path))
                .unwrap_or_else(|| home.join(".claude"));
            default_dirs(&home, &config)
        }
    };
    installed_in(&dirs)
}

fn untagged_skills<'a>(page: &pages::Page, names: &'a [String]) -> Vec<&'a str> {
    if !matches!(page.kind.as_str(), "knowledge" | "followups") {
        return vec![];
    }
    let tags: Vec<_> = page
        .tags
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    names
        .iter()
        .filter_map(|name| {
            if name == "claude-wiki" || tags.contains(name) {
                return None;
            }
            // Hyphens and underscores belong to identifiers, unlike regex \b.
            let mention = Regex::new(&format!(
                r"(?i)(?:^|[^\p{{L}}\p{{N}}_-]){}(?:$|[^\p{{L}}\p{{N}}_-])",
                regex::escape(name)
            ))
            .unwrap();
            (mention.is_match(&page.title) || mention.is_match(&page.body)).then_some(name.as_str())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_errors_block_check_and_sync_without_a_commit() -> Result<()> {
        let mut wiki = crate::usage::tests::Wiki::new()?;
        for text in [
            "body",
            "---\ntitle: broken: title\n---\n[[knowledge/missing]]",
            "---\ntitle: Test\n---\nbody",
        ] {
            wiki.page("pages/knowledge/broken.md", text)?;
            assert_eq!(check(&wiki.root, &wiki.index, &[], false, 3)?, 2);
            assert_eq!(sync(&wiki.root, &wiki.index, &[], None)?, 2);
            assert!(git(&wiki.root, &["rev-parse", "--verify", "HEAD"]).is_err());
        }
        let page = pages::parse("---\ntitle: broken: title\n---\n", "pages/knowledge/x.md");
        assert!(page.problems[0].contains("quote the title: title: \"broken: title\""));
        Ok(())
    }

    #[test]
    fn shrink_requires_both_thresholds_and_counts_characters() {
        assert_eq!(
            shrink(&"한".repeat(4000), &"글".repeat(2000)),
            Some((4000, 2000))
        );
        assert_eq!(shrink(&"x".repeat(4000), &"x".repeat(2400)), None);
        assert_eq!(shrink(&"x".repeat(2000), &"x".repeat(500)), None);
        assert_eq!(shrink("small", &"x".repeat(4000)), None);
    }

    #[test]
    fn mentions_use_whole_names_and_tags_including_code() {
        let names = vec!["claude-wiki".into(), "codex-implement".into()];
        let mut page = pages::Page {
            kind: "knowledge".into(),
            ..pages::Page::default()
        };
        for body in [
            "codex-implement-x",
            "x-codex-implement",
            "codex-implement_x",
            "xcodex-implement",
            "codex-implement1",
            "한codex-implement",
            "claude-wiki",
        ] {
            page.body = body.into();
            assert!(untagged_skills(&page, &names).is_empty(), "{body}");
        }
        for body in [
            "codex-implement",
            "The CoDeX-ImPlEmEnT skill.",
            "```bash\n/codex-implement\n```",
        ] {
            page.body = body.into();
            assert_eq!(untagged_skills(&page, &names), ["codex-implement"]);
        }
        page.body.clear();
        page.title = "About codex-implement".into();
        page.kind = "followups".into();
        assert_eq!(untagged_skills(&page, &names), ["codex-implement"]);
        page.tags = "CODEX-IMPLEMENT".into();
        assert!(untagged_skills(&page, &names).is_empty());
        page.tags = "codex-implement-x".into();
        assert_eq!(untagged_skills(&page, &names), ["codex-implement"]);
        for kind in ["projects", "history"] {
            page.kind = kind.into();
            assert!(untagged_skills(&page, &names).is_empty());
        }
    }

    #[test]
    fn discovery_includes_config_and_plugin_versions() -> std::io::Result<()> {
        let root = env::temp_dir().join(format!("cw-skills-{}", std::process::id()));
        let config = root.join("config");
        for skill in [
            "config/skills/demo-skill",
            "config/skills/claude-wiki",
            ".claude/plugins/cache/vendor/plugin/v1/skills/plugin-skill",
            ".claude/plugins/cache/vendor/plugin/v2/skills/demo-skill",
        ] {
            let path = root.join(skill);
            fs::create_dir_all(&path)?;
            fs::write(path.join("SKILL.md"), "skill")?;
        }
        fs::create_dir_all(config.join("skills/not-installed"))?;
        assert_eq!(
            installed_in(&default_dirs(&root, &config)),
            ["demo-skill", "plugin-skill"]
        );
        assert_eq!(
            installed_in(&[config.join("skills"), root.join("missing")]),
            ["demo-skill"]
        );
        fs::remove_dir_all(root)
    }
}

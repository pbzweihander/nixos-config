use crate::{
    bulletin::clean_identifier,
    index::{COLUMNS, JOIN, Row},
    pages, search,
};
use anyhow::Result;
use clap::Args as ClapArgs;
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// Largest `.git` or `commondir` pointer file read while resolving the project.
const POINTER_BYTES: u64 = 4096;
/// Largest overview page rendered; a larger one is reported instead of read.
const OVERVIEW_BYTES: u64 = 1024 * 1024;

#[derive(ClapArgs)]
pub struct Args {
    #[arg(long, value_enum, default_value = "claude")]
    pub harness: crate::hook_payload::Harness,
    #[arg(short, default_value_t = 10)]
    pub n: usize,
    #[arg(long, env = crate::compat::env_name("AGENT_WIKI_CONTEXT_BUDGET", "CLAUDE_WIKI_CONTEXT_BUDGET"), default_value_t = 6000)]
    pub budget: usize,
    #[arg(long, env = crate::compat::env_name("AGENT_WIKI_OVERVIEW_BUDGET", "CLAUDE_WIKI_OVERVIEW_BUDGET"), default_value_t = 3000)]
    pub overview_budget: usize,
}

pub fn overview_budget() -> usize {
    crate::compat::var("AGENT_WIKI_OVERVIEW_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000)
}

/// The repository name for `dir`: the directory holding the main `.git` of the
/// repository that contains it, shared by all its worktrees, or a bare repository's
/// name without `.git`. `None` outside a repository or for a missing directory.
///
/// Hooks call this at session start, so it reads the repository layout directly
/// instead of spawning `git rev-parse --git-common-dir`, which can stall on a slow
/// filesystem or configuration hook. Like that command it walks up from `dir`,
/// follows a `gitdir:` file and `commondir`, and resolves symlinks; unlike it, it
/// ignores `GIT_DIR` and other `GIT_*` overrides inherited from the caller.
pub fn current_project(dir: &Path) -> Option<String> {
    let start = fs::canonicalize(dir).ok()?;
    if !start.is_dir() {
        return None;
    }
    let common = start
        .ancestors()
        .find_map(git_dir_at)
        .and_then(common_dir)?;
    if common.file_name()? == ".git" {
        Some(common.parent()?.file_name()?.to_string_lossy().into_owned())
    } else {
        let name = common.file_name()?.to_string_lossy();
        Some(name.strip_suffix(".git").unwrap_or(&name).to_owned())
    }
}

/// Text of a small regular file (following symlinks); never opens a FIFO or device.
fn pointer_file(path: &Path) -> Option<String> {
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(POINTER_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= POINTER_BYTES).then_some(text)
}

/// Whether `path` looks like a git directory: a `HEAD` file and either its own
/// object store or a `commondir` pointer (a linked worktree's git directory).
fn is_git_dir(path: &Path) -> bool {
    fs::metadata(path.join("HEAD")).is_ok_and(|m| m.is_file())
        && (fs::metadata(path.join("commondir")).is_ok_and(|m| m.is_file())
            || (path.join("objects").is_dir() && path.join("refs").is_dir()))
}

/// The git directory that `dir` belongs to directly: its `.git` directory, the
/// target of its `.git` file, or `dir` itself when it is a git directory.
fn git_dir_at(dir: &Path) -> Option<PathBuf> {
    let dotgit = dir.join(".git");
    match fs::metadata(&dotgit) {
        Ok(meta) if meta.is_dir() => is_git_dir(&dotgit).then_some(dotgit),
        Ok(meta) if meta.is_file() => {
            let text = pointer_file(&dotgit)?;
            let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
            let gitdir = dir.join(target);
            is_git_dir(&gitdir).then_some(gitdir)
        }
        _ => is_git_dir(dir).then(|| dir.to_path_buf()),
    }
}

/// The common git directory of `gitdir`, resolved like `--path-format=absolute`.
fn common_dir(gitdir: PathBuf) -> Option<PathBuf> {
    let common = match pointer_file(&gitdir.join("commondir")) {
        Some(text) => gitdir.join(text.lines().next()?.trim()),
        None => gitdir,
    };
    fs::canonicalize(common).ok()
}

/// The overview page as it is on disk now, or the category that keeps it hidden.
///
/// The file must be a regular file of at most [`OVERVIEW_BYTES`]. The scan covers the
/// text as written (what the index scans), the body joined into one line (a phrase
/// split across lines), the decoded frontmatter values, and a `summary:` that
/// decodes into blocked text.
fn read_overview(path: &Path, rel: &str) -> std::result::Result<pages::Page, String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "unreadable".to_owned())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    let mut bytes = vec![];
    fs::File::open(path)
        .and_then(|file| file.take(OVERVIEW_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| "unreadable".to_owned())?;
    if bytes.len() as u64 > OVERVIEW_BYTES {
        return Err("too large".into());
    }
    let raw = String::from_utf8_lossy(&bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let page = pages::parse(&raw, rel);
    let metadata = [page.title.as_str(), &page.status, &page.project, &page.tags].join("\n");
    for text in [
        raw.as_str(),
        &pages::one_line(&page.body),
        &metadata,
        &pages::one_line(&metadata),
    ] {
        if let Some(finding) = crate::scan::scan(text).first() {
            return Err(finding.category.into());
        }
    }
    if page.summary_blocked {
        let category = page
            .problems
            .iter()
            .find_map(|p| p.split_once("the scanner blocks: "))
            .and_then(|(_, rest)| rest.split_once(':'))
            .map_or("summary", |(category, _)| category);
        return Err(category.to_owned());
    }
    Ok(page)
}

pub fn number(n: usize) -> String {
    let s = n.to_string();
    s.chars()
        .enumerate()
        .fold(String::new(), |mut out, (i, c)| {
            if i != 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
            out
        })
}

fn cap(body: &str, budget: usize) -> (String, usize) {
    let total = body.chars().count();
    if total <= budget {
        return (body.into(), 0);
    }
    let prefix: String = body.chars().take(budget).collect();
    // A single long line still needs a bounded preview.
    let shown = prefix
        .rfind('\n')
        .map_or(prefix.as_str(), |i| &prefix[..i])
        .to_owned();
    let remaining = total - shown.chars().count();
    (shown, remaining)
}

struct Section {
    name: &'static str,
    heading: String,
    items: Vec<String>,
    original: usize,
}

fn assemble(pinned: &str, sections: &[Section], budget: usize) -> (String, usize) {
    let mut text = pinned.to_owned();
    for section in sections {
        if section.items.is_empty() {
            continue;
        }
        text.push('\n');
        if section.name == "tags" {
            text.push_str(&format!("Tags: {}\n", section.items.join(", ")));
        } else {
            text.push_str(&format!(
                "{}\n{}\n",
                section.heading,
                section.items.join("\n")
            ));
        }
    }
    let trimmed = sections
        .iter()
        .filter(|s| s.items.len() < s.original)
        .map(|s| {
            if s.items.is_empty() {
                s.name.to_owned()
            } else {
                format!(
                    "{} ({} of {})",
                    s.name,
                    s.original - s.items.len(),
                    s.original
                )
            }
        })
        .collect::<Vec<_>>();
    let suffix = if trimmed.is_empty() {
        String::new()
    } else {
        format!("; trimmed: {}", trimmed.join(", "))
    };
    // Include the footer itself and its newline in the advertised character count.
    let base = text.chars().count();
    let mut used = base;
    loop {
        let footer = format!(
            "[context {}/{} chars{suffix}]\n",
            number(used),
            number(budget)
        );
        let next = base + footer.chars().count();
        if next == used {
            text.push_str(&footer);
            return (text, used);
        }
        used = next;
    }
}

fn budgeted(pinned: &str, sections: &mut [Section], budget: usize) -> (String, usize) {
    loop {
        let (text, used) = assemble(pinned, sections, budget);
        if used <= budget {
            return (text, used);
        }
        if let Some(section) = sections.iter_mut().rev().find(|s| !s.items.is_empty()) {
            section.items.pop();
        } else {
            return (text, used);
        }
    }
}

fn hook_input() -> Option<serde_json::Value> {
    use std::{
        io::{self, IsTerminal, Read},
        sync::mpsc,
        thread,
        time::Duration,
    };
    if io::stdin().is_terminal() {
        return None;
    }
    let (send, receive) = mpsc::channel();
    thread::Builder::new()
        .spawn(move || {
            let mut input = String::new();
            let id = io::stdin()
                .take(1024 * 1024)
                .read_to_string(&mut input)
                .ok()
                .and_then(|_| serde_json::from_str::<serde_json::Value>(&input).ok());
            let _ = send.send(id);
        })
        .ok()?;
    receive
        .recv_timeout(Duration::from_millis(200))
        .ok()
        .flatten()
}

/// Present only when a harness supplied hook input, never for a manual context call.
pub(crate) struct HookContext<'a> {
    pub session: Option<&'a str>,
    pub cwd: Option<&'a str>,
}

pub fn run(root: &Path, db: &Connection, args: &Args) -> Result<()> {
    let input = hook_input();
    let hook = input
        .as_ref()
        .filter(|v| {
            v.is_object()
                && v["hook_event_name"]
                    .as_str()
                    .is_none_or(|event| event == "SessionStart")
        })
        .map(|v| HookContext {
            session: v["session_id"].as_str(),
            cwd: v["cwd"].as_str(),
        });
    // Resolve the project from the payload's cwd, as Antigravity does: the hook
    // process may start in another directory than the session it serves.
    let cwd = hook
        .as_ref()
        .and_then(|hook| hook.cwd)
        .filter(|cwd| !cwd.is_empty())
        .map_or(Path::new("."), Path::new);
    let text = render(root, db, args, cwd, hook)?;
    print!("{}", args.harness.context("SessionStart", &text));
    Ok(())
}

pub(crate) fn render(
    root: &Path,
    db: &Connection,
    args: &Args,
    cwd: &Path,
    hook: Option<HookContext<'_>>,
) -> Result<String> {
    let mut injected = vec![];
    let mut listed = std::collections::HashMap::new();
    let total: i64 = db.query_row("select count(*) from files", [], |r| r.get(0))?;
    let mut pinned = format!(
        "Shared wiki: {total} pages under {}/. Search it with `agent-wiki search <terms>` before non-trivial work; load the agent-wiki skill before writing to it.\n",
        root.display()
    );
    let project = current_project(cwd);
    let overview = format!("pages/projects/{}.md", project.as_deref().unwrap_or("None"));
    // The project is a directory name and the overview path derives from it: both
    // are shown only as cleaned identifiers, never verbatim.
    let shown_overview = clean_identifier(&overview);
    let mut queries: Vec<(&str, String, String, Vec<Value>)> = vec![];
    if let Some(project) = &project {
        let shown_project = clean_identifier(project);
        pinned += &format!("Current project: {shown_project}\n");
        let row = db
            .query_row(
                &format!("select {COLUMNS} {JOIN} where fts.path = ?"),
                [&overview],
                Row::read,
            )
            .optional()?;
        if let Some(row) = row {
            injected.push(overview.clone());
            // The index may predate the bytes on disk (an edit that kept size and
            // mtime, or a write after the refresh): scan what is read now.
            let current = if row.blocked.is_empty() {
                read_overview(&root.join(&overview), &overview)
            } else {
                Err(row.blocked.clone())
            };
            match current {
                Err(category) => {
                    pinned += &format!("\n{shown_overview}: [BLOCKED: {category}; fix the page]\n");
                }
                Ok(page) => {
                    let (shown, remaining) = cap(page.body.trim(), args.overview_budget);
                    pinned += &format!(
                        "\nProject overview ({shown_overview}) [{}/{} chars]\n{shown}\n",
                        number(shown.chars().count()),
                        number(args.overview_budget)
                    );
                    if remaining > 0 {
                        pinned += &format!(
                            "[... {} more chars in the page; shorten the page to fit]\n",
                            number(remaining)
                        );
                    }
                }
            }
        } else {
            pinned += &format!(
                "No overview page for {shown_project} yet; write {shown_overview} once you have explored the project.\n"
            );
        }
        queries.push((
            "follow-ups",
            format!("Open follow-ups for {shown_project}:"),
            "fts.type = 'followups' and fts.status = 'open' and fts.project = ?".into(),
            vec![project.clone().into()],
        ));
        queries.push((
            "knowledge",
            format!("Knowledge pages for {shown_project}:"),
            "fts.type = 'knowledge' and fts.project = ?".into(),
            vec![project.clone().into()],
        ));
    }
    queries.push((
        "recently updated",
        "Recently updated:".into(),
        "not (fts.type = 'followups' and fts.status = 'done')".into(),
        vec![],
    ));
    let mut seen = HashSet::from([overview]);
    let mut sections = vec![];
    for (name, heading, where_sql, mut params) in queries {
        params.push(Value::Integer(if name == "recently updated" {
            args.n.saturating_mul(3)
        } else {
            args.n
        } as i64));
        let mut stmt = db.prepare(&format!(
            "select {COLUMNS} {JOIN} where {where_sql} order by files.mtime_ns desc limit ?"
        ))?;
        let rows = stmt
            .query_map(params_from_iter(params), Row::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut items = vec![];
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|r| !seen.contains(&r.path))
            .take(args.n)
            .collect();
        for row in rows {
            seen.insert(row.path.clone());
            if matches!(name, "follow-ups" | "knowledge") {
                listed
                    .entry(name)
                    .or_insert_with(Vec::new)
                    .push(row.path.clone());
            }
            let path = clean_identifier(&row.path);
            if !row.blocked.is_empty() {
                items.push(row.blocked_line(&path));
            } else {
                let mut extra = String::new();
                for x in [
                    row.status.as_str(),
                    if Some(&row.project) != project.as_ref() {
                        row.project.as_str()
                    } else {
                        ""
                    },
                ] {
                    if !x.is_empty() {
                        extra += &format!(", {}", clean_identifier(x));
                    }
                }
                if row.backlinks > 0 && matches!(row.kind.as_str(), "knowledge" | "followups") {
                    extra += &format!(", linked by {}", row.backlinks);
                }
                items.push(format!(
                    "- {path}: {} ({}, {}{extra})",
                    clean_identifier(&row.title),
                    row.kind,
                    search::updated(row.mtime)
                ));
            }
        }
        sections.push(Section {
            name,
            heading,
            original: items.len(),
            items,
        });
    }
    let items = search::tag_counts(db)?
        .into_iter()
        .take(30)
        .map(|(tag, n)| format!("{} ({n})", clean_identifier(&tag)))
        .collect::<Vec<_>>();
    sections.push(Section {
        name: "tags",
        heading: String::new(),
        original: items.len(),
        items,
    });
    // Announce bulletins here so the session's first prompt does not repeat them.
    if let Some(hook) = &hook
        && let Some(note) = crate::bulletin::hook(
            root,
            db,
            hook.cwd,
            hook.session.unwrap_or_default(),
            (args.budget / 5).min(crate::bulletin::MAX_CHARS),
        )
    {
        pinned += &format!("\n{note}\n");
    }
    if let Some(hook) = &hook
        && let Some(note) = crate::bot_guard::session_start(
            root,
            hook.cwd,
            args.budget
                .saturating_sub(pinned.chars().count() + 100)
                .min(1500),
        )
    {
        pinned += &format!("\n{note}\n");
    }
    let (text, used) = budgeted(&pinned, &mut sections, args.budget);
    if used > args.budget {
        eprintln!(
            "warning: context is {used} chars; protected header and overview exceed the {} char budget",
            args.budget
        );
    }
    if let Some(session) = hook.and_then(|hook| hook.session) {
        for section in &sections {
            if let Some(paths) = listed.get(section.name) {
                injected.extend(paths.iter().take(section.items.len()).cloned());
            }
        }
        crate::usage::log(root, session, "context", &injected);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// The project name the previous implementation derived from git itself.
    fn from_git(dir: &Path) -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let common = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
        if common.file_name()? == ".git" {
            Some(common.parent()?.file_name()?.to_string_lossy().into_owned())
        } else {
            let name = common.file_name()?.to_string_lossy();
            Some(name.strip_suffix(".git").unwrap_or(&name).to_owned())
        }
    }

    #[test]
    fn current_project_matches_git_without_running_it() {
        let root = Scratch(std::env::temp_dir().join(format!(
            "cw-project-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )));
        let base = &root.0;
        fs::create_dir_all(base.join("main/sub/deeper")).unwrap();
        fs::create_dir_all(base.join("plain")).unwrap();
        let main = base.join("main");
        git(&main, &["init", "-q"]);
        fs::write(main.join("file"), "x").unwrap();
        git(&main, &["add", "file"]);
        git(&main, &["commit", "-qm", "init"]);
        let worktree = base.join("worktrees/feature-x");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature-x",
                worktree.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(worktree.join("nested")).unwrap();
        git(
            base,
            &["clone", "-q", "--bare", main.to_str().unwrap(), "bare.git"],
        );
        let bare_worktree = base.join("bare-wt");
        git(
            &base.join("bare.git"),
            &["worktree", "add", "-q", bare_worktree.to_str().unwrap()],
        );
        std::os::unix::fs::symlink(&main, base.join("alias")).unwrap();
        let cases = [
            (main.clone(), Some("main")),
            (main.join("sub/deeper"), Some("main")),
            (main.join(".git"), Some("main")),
            (main.join(".git/objects"), Some("main")),
            (worktree.clone(), Some("main")),
            (worktree.join("nested"), Some("main")),
            (base.join("bare.git"), Some("bare")),
            (bare_worktree.clone(), Some("bare")),
            (base.join("alias/sub"), Some("main")),
            (base.join("plain"), None),
            (base.join("missing"), None),
            (main.join("file"), None),
        ];
        for (dir, expected) in cases {
            assert_eq!(current_project(&dir).as_deref(), expected, "{dir:?}");
            assert_eq!(current_project(&dir), from_git(&dir), "{dir:?}");
        }
        // A FIFO in place of a `.git` file is skipped, never opened.
        let fifo_dir = base.join("plain/fifo");
        fs::create_dir(&fifo_dir).unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(fifo_dir.join(".git"))
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(current_project(&fifo_dir), None);
        // A broken `gitdir:` pointer is not a repository either.
        let broken = base.join("plain/broken");
        fs::create_dir(&broken).unwrap();
        fs::write(broken.join(".git"), "gitdir: /nonexistent/x\n").unwrap();
        assert_eq!(current_project(&broken), None);
        assert_eq!(from_git(&broken), None);
    }

    #[test]
    fn unicode_and_line_boundaries() {
        assert_eq!(cap("abc\ndefgh", 7), ("abc".into(), 6));
        assert_eq!(cap("한글문장", 2), ("한글".into(), 2));
        assert_eq!(number(1234567), "1,234,567");
    }
    #[test]
    fn budget_counts_footer_and_drops_low_priority_first() {
        let mut sections = vec![
            Section {
                name: "follow-ups",
                heading: "Follow-ups:".into(),
                original: 1,
                items: vec!["Important".into()],
            },
            Section {
                name: "tags",
                heading: String::new(),
                original: 1,
                items: vec!["x".repeat(200)],
            },
        ];
        let (text, used) = budgeted("Header\n", &mut sections, 100);
        assert!(text.contains("Important"));
        assert!(text.contains("trimmed: tags"));
        assert_eq!(text.chars().count(), used);
        assert!(used <= 100);
    }
}

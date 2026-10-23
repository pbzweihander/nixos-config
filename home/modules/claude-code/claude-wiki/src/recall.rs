use anyhow::Result;
use clap::Args as ClapArgs;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::File,
    io,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

const WINDOW: usize = 6;
const MAX_WINDOW_PAGES: usize = 3;
const MAX_WINDOWS: usize = 80;
static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());
static PAGE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"pages/(?:knowledge|projects|followups|history)/[A-Za-z0-9._-]+\.md").unwrap()
});
static ERRORS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(error|failed|failure|cannot|can't|not permitted|permission denied|not found|no such|panic|traceback|exception|fatal|refused|timed out|invalid|unrecognized|unknown option|read-only file system)\b").unwrap()
});

#[derive(ClapArgs)]
pub struct Args {
    /// Minimum match score: each six-word window counts 1, each gated 3–5-word
    /// phrase counts 2. Default 2 means seven consecutive matching words
    /// (overlapping windows), two separate six-word runs, or one gated short phrase.
    #[arg(long, env = "CLAUDE_WIKI_RECALL_MIN_WINDOWS", default_value_t = 2)]
    min_windows: usize,
    #[arg(long, env = "CLAUDE_WIKI_RECALL_MAX_SKILL_PAGES", default_value_t = 5)]
    max_skill_pages: usize,
    #[arg(long, env = "CLAUDE_WIKI_RECALL_MAX_PAGES", default_value_t = 2)]
    max_pages: usize,
    #[arg(long)]
    include_history: bool,
    #[arg(long)]
    explain: bool,
}

#[derive(ClapArgs)]
pub struct SkillPagesArgs {
    skill: String,
    #[arg(long)]
    json: bool,
    #[arg(short, default_value_t = 5)]
    n: usize,
    #[arg(long, default_value = ".")]
    cwd: PathBuf,
    #[arg(long)]
    session: Option<String>,
    #[arg(long, value_parser = crate::usage::source)]
    source: Option<String>,
}

#[derive(Default, Deserialize)]
struct Hook {
    session_id: Option<String>,
    transcript_path: Option<String>,
    cwd: Option<String>,
    #[serde(default)]
    hook_event_name: String,
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
    #[serde(default)]
    error: Value,
    #[serde(default)]
    is_interrupt: bool,
}

fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for part in cwd.join(crate::expand_home(path)).components() {
        match part {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            _ => normalized.push(part.as_os_str()),
        }
    }
    normalized
}

fn wiki_operation(root: &Path, hook: &Hook) -> bool {
    if hook.tool_name == "Bash" {
        // Recognize common command prefixes without trying to interpret shell code.
        static WIKI_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r#"(?x)^\s*(?:
                [A-Za-z_][A-Za-z_0-9]*=(?:[^\s'";|&]+|'[^']*'|"[^"]*")*\s+
                | cd\s+(?:[^'";&]|'[^']*'|"[^"]*")+&&\s*
            )*claude-wiki(?:\s|$|[;&|])"#,
            )
            .unwrap()
        });
        // Commands name the wiki the way people type it, not only canonically.
        let mut spellings = vec![root.to_string_lossy().into_owned()];
        if let Ok(rel) = root.strip_prefix(crate::home()) {
            let rel = rel.to_string_lossy();
            spellings.extend([
                format!("~/{rel}"),
                format!("$HOME/{rel}"),
                format!("${{HOME}}/{rel}"),
            ]);
        }
        return hook.tool_input["command"].as_str().is_some_and(|command| {
            spellings.iter().any(|s| command.contains(s.as_str())) || WIKI_COMMAND.is_match(command)
        });
    }
    let key = match hook.tool_name.as_str() {
        "Edit" | "Write" | "MultiEdit" => "file_path",
        "NotebookEdit" => "notebook_path",
        _ => return false,
    };
    hook.tool_input[key].as_str().is_some_and(|path| {
        let cwd = hook.cwd.as_deref().map(Path::new).unwrap_or(root);
        let path = absolute(Path::new(path), cwd);
        path.starts_with(root) || path.canonicalize().is_ok_and(|path| path.starts_with(root))
    })
}

#[derive(Debug, Serialize)]
struct ErrorWindow {
    phrase: String,
    short: bool,
    df: usize,
    kept: bool,
    reason: Option<&'static str>,
}

fn error_windows(error: &str, home: &Path) -> Vec<ErrorWindow> {
    let home = home.to_string_lossy().to_lowercase();
    let home_words: HashSet<_> = WORDS.find_iter(&home).map(|m| m.as_str()).collect();
    let mut seen = HashSet::new();
    let mut windows = vec![];
    // Start at the end so repeated diagnostics cannot displace the final failure.
    'lines: for line in error
        .lines()
        .rev()
        .take(200)
        .filter(|line| ERRORS.is_match(line))
    {
        let line = line.to_lowercase();
        let words: Vec<_> = WORDS.find_iter(&line).map(|m| m.as_str()).collect();
        for segment in words.rsplit(|word| {
            word.chars().all(char::is_numeric)
                || (word.len() >= 7
                    && word.bytes().all(|c| c.is_ascii_hexdigit())
                    && word.bytes().any(|c| c.is_ascii_digit()))
                || home_words.contains(word)
        }) {
            if segment.len() < 3 {
                continue;
            }
            for words in segment.windows(WINDOW.min(segment.len())).rev() {
                let phrase = words.join(" ");
                if seen.insert(phrase.clone()) {
                    windows.push(ErrorWindow {
                        phrase,
                        short: words.len() < WINDOW,
                        df: 0,
                        kept: false,
                        reason: None,
                    });
                    if windows.len() == MAX_WINDOWS {
                        break 'lines;
                    }
                }
            }
        }
    }
    windows.reverse();
    windows
}

fn quote(phrase: &str) -> String {
    format!("\"{}\"", phrase.replace('"', "\"\""))
}

#[derive(Debug, Serialize)]
struct Candidate {
    path: String,
    title: String,
    score: usize,
    project_match: bool,
    qualified: bool,
    seen: bool,
    #[serde(skip)]
    mtime: i64,
}

impl Candidate {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            path: row.get(0)?,
            title: row.get(1)?,
            mtime: row.get(2)?,
            score: 0,
            project_match: false,
            qualified: false,
            seen: false,
        })
    }
}

fn error_candidates(
    db: &Connection,
    args: &Args,
    windows: &mut [ErrorWindow],
) -> Result<Vec<Candidate>> {
    if windows.is_empty() {
        return Ok(vec![]);
    }
    let queries: Vec<_> = windows.iter().map(|w| quote(&w.phrase)).collect();
    // Count in a batch before loading page metadata: common error phrases can
    // match the entire wiki, but only windows shared by at most three pages matter.
    let filter = "files.blocked = '' and (? or fts.type != 'history')";
    let mut counts = db.prepare(&format!(
        "select q.key, count(*)
        from json_each(?) q cross join fts join files on files.id = fts.rowid
        where fts match q.value and {filter} group by q.key"
    ))?;
    for result in counts.query_map(
        params![serde_json::to_string(&queries)?, args.include_history],
        |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
    )? {
        let (i, df) = result?;
        windows[i].df = df;
    }
    let mut token_counts = db.prepare(&format!(
        "select count(*) from fts join files on files.id = fts.rowid
        where fts match ? and {filter}"
    ))?;
    for window in windows.iter_mut() {
        window.reason = if window.df == 0 {
            Some("no matches")
        } else if window.df > MAX_WINDOW_PAGES {
            Some("shared by too many pages")
        } else if window.short {
            let mut rare = false;
            for token in window.phrase.split_whitespace() {
                let df: i64 = token_counts
                    .query_row(params![quote(token), args.include_history], |r| r.get(0))?;
                if (1..=MAX_WINDOW_PAGES as i64).contains(&df) {
                    rare = true;
                    break;
                }
            }
            (!rare).then_some("no rare token")
        } else {
            None
        };
        window.kept = window.reason.is_none();
    }
    let queries: Vec<_> = windows
        .iter()
        .filter(|w| w.kept)
        .map(|w| json!({"phrase": quote(&w.phrase), "weight": if w.short { 2 } else { 1 }}))
        .collect();
    if queries.is_empty() {
        return Ok(vec![]);
    }
    let mut matches = db.prepare(&format!(
        "select files.path, fts.title, files.mtime_ns, sum(json_extract(q.value, '$.weight'))
        from json_each(?) q cross join fts join files on files.id = fts.rowid
        where fts match json_extract(q.value, '$.phrase') and {filter} group by files.id"
    ))?;
    let mut pages = matches
        .query_map(
            params![serde_json::to_string(&queries)?, args.include_history],
            |r| {
                let mut page = Candidate::read(r)?;
                page.score = r.get::<_, i64>(3)? as usize;
                page.qualified = page.score >= args.min_windows;
                Ok(page)
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    pages.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.mtime.cmp(&a.mtime))
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(pages)
}

fn skill_names(skill: &str) -> Vec<String> {
    let name = skill.trim().to_lowercase();
    let suffix = name.rsplit(':').next().unwrap_or_default();
    if name.is_empty() || suffix == "claude-wiki" {
        return vec![];
    }
    if suffix != name && !suffix.is_empty() {
        vec![name.clone(), suffix.to_owned()]
    } else {
        vec![name]
    }
}

fn skill_candidates(
    db: &Connection,
    names: &[String],
    project: Option<&str>,
) -> Result<Vec<Candidate>> {
    let mut query = db.prepare("select files.path, fts.title, files.mtime_ns, fts.tags, fts.project
        from fts join files on files.id = fts.rowid
        where files.blocked = '' and (fts.type = 'knowledge' or (fts.type = 'followups' and fts.status = 'open'))
        and (fts.project = '' or fts.project = ?)")?;
    let mut pages = vec![];
    for result in query.query_map([project.unwrap_or_default()], |r| {
        Ok((
            Candidate::read(r)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })? {
        let (mut page, tags, page_project) = result?;
        // Splitting tags keeps plugin colons and hyphens literal; LIKE would treat
        // underscores and percent signs as wildcards, and FTS would split the name.
        if tags
            .split_whitespace()
            .any(|tag| names.contains(&tag.to_lowercase()))
        {
            page.project_match = project.is_some_and(|p| p == page_project);
            page.qualified = true;
            pages.push(page);
        }
    }
    pages.sort_by(|a, b| {
        b.project_match
            .cmp(&a.project_match)
            .then_with(|| b.mtime.cmp(&a.mtime))
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(pages)
}

pub fn mark_seen(db: &Connection, session: &str, paths: &[String]) -> Result<()> {
    let mut insert =
        db.prepare("insert or ignore into recall_seen(session_id, path) values (?, ?)")?;
    for path in paths {
        // The embedding daemon also uses the short wiki-relative ID, "knowledge/foo".
        let path = if path.starts_with("pages/") {
            path.clone()
        } else {
            format!("pages/{path}.md")
        };
        insert.execute(params![session, path])?;
    }
    Ok(())
}

fn session_seen(db: &Connection, id: &str) -> Result<HashSet<String>> {
    let seen = db
        .prepare("select path from recall_seen where session_id = ?")?
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(seen)
}

pub fn transcript_seen(
    db: &Connection,
    id: &str,
    transcript_path: Option<&str>,
) -> Result<HashSet<String>> {
    let mut seen = session_seen(db, id)?;
    let mut offset: i64 = db
        .query_row(
            "select offset from recall_state where session_id = ?",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let mut added = vec![];
    if let Some(path) = transcript_path {
        match File::open(path) {
            Ok(file) => {
                if file.metadata()?.len() < offset as u64 {
                    offset = 0;
                }
                let paths: HashSet<String> = db
                    .prepare("select path from files")?
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                offset = i64::try_from(crate::sessions::complete_lines(
                    file,
                    offset as u64,
                    |line| {
                        let Ok(value) = serde_json::from_slice::<Value>(line) else {
                            return Ok(());
                        };
                        if value["type"] == "assistant"
                            && let Some(parts) = value["message"]["content"].as_array()
                        {
                            for part in parts.iter().filter(|p| p["type"] == "tool_use") {
                                // One pass over the input rather than one per page: a
                                // resumed session hands over its whole transcript at once.
                                // An absolute path contains its relative form.
                                let input = part["input"].to_string();
                                for m in PAGE_PATH.find_iter(&input) {
                                    let rel = m.as_str();
                                    if paths.contains(rel) && seen.insert(rel.to_owned()) {
                                        added.push(rel.to_owned());
                                    }
                                }
                            }
                        }
                        Ok(())
                    },
                )?)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("claude-wiki: cannot read recall transcript: {e}"),
        }
    }
    let tx = db.unchecked_transaction()?;
    mark_seen(&tx, id, &added)?;
    tx.execute("insert into recall_state(session_id, offset) values (?, ?) on conflict(session_id) do update set offset = excluded.offset", params![id, offset])?;
    tx.commit()?;
    Ok(seen)
}

fn decide(root: &Path, db: &Connection, hook: &Hook, args: &Args) -> Result<Option<Value>> {
    let seen = if args.explain {
        HashSet::new()
    } else {
        transcript_seen(
            db,
            hook.session_id.as_deref().unwrap_or_default(),
            hook.transcript_path.as_deref(),
        )?
    };
    let mut mode = None;
    let mut windows = vec![];
    let mut names = vec![];
    let mut project = None;
    let mut candidates = vec![];
    if !wiki_operation(root, hook) {
        match hook.hook_event_name.as_str() {
            "PreToolUse" if hook.tool_name == "Skill" => {
                names = skill_names(hook.tool_input["skill"].as_str().unwrap_or_default());
                if !names.is_empty() {
                    mode = Some("skill");
                    project = hook
                        .cwd
                        .as_deref()
                        .filter(|cwd| !cwd.is_empty())
                        .and_then(|cwd| crate::context::current_project(Path::new(cwd)));
                    candidates = skill_candidates(db, &names, project.as_deref())?;
                }
            }
            "PostToolUseFailure" if !hook.is_interrupt => {
                mode = Some("error");
                windows = error_windows(hook.error.as_str().unwrap_or_default(), &crate::home());
                candidates = error_candidates(db, args, &mut windows)?;
            }
            _ => {}
        }
    }
    for page in &mut candidates {
        page.seen = seen.contains(&page.path);
    }
    let limit = if mode == Some("skill") {
        args.max_skill_pages
    } else {
        args.max_pages
    };
    let selected: Vec<_> = candidates
        .iter()
        .filter(|p| p.qualified && !p.seen)
        .take(limit)
        .collect();
    let paths: Vec<_> = selected.iter().map(|p| p.path.clone()).collect();
    if args.explain {
        return Ok(Some(
            json!({"event": hook.hook_event_name, "tool_name": hook.tool_name,
            "mode": mode, "windows": windows, "skill_names": names, "project": project,
            "candidates": candidates.iter().take(10).collect::<Vec<_>>(), "selected": paths}),
        ));
    }
    if selected.is_empty() {
        return Ok(None);
    }
    let pages = selected
        .iter()
        .map(|p| {
            format!(
                "{} ({})",
                root.join(&p.path).display(),
                p.title.chars().take(120).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let message = if mode == Some("skill") {
        let skill = hook.tool_input["skill"].as_str().unwrap_or_default();
        format!(
            "claude-wiki: wiki pages about the {skill} skill: {pages}. Read any that bears on this task; otherwise ignore this note and do not reply to it."
        )
    } else {
        format!(
            "claude-wiki: pages that may explain this {} failure: {pages}. Read one if it is relevant to what you are about to do; otherwise ignore this note and do not reply to it.",
            hook.tool_name
        )
    };
    mark_seen(db, hook.session_id.as_deref().unwrap_or_default(), &paths)?;
    crate::usage::log(
        root,
        hook.session_id.as_deref().unwrap_or_default(),
        if mode == Some("skill") {
            "recall-skill"
        } else {
            "recall-failure"
        },
        &paths,
    );
    Ok(Some(
        json!({"hookSpecificOutput": {"hookEventName": hook.hook_event_name, "additionalContext": message}}),
    ))
}

pub fn run(root: &Path, db: &Connection, args: &Args) -> Result<()> {
    let input = io::read_to_string(io::stdin())?;
    if input.trim().is_empty() {
        return Ok(());
    }
    let hook = serde_json::from_str(&input)?;
    let root = root.canonicalize()?;
    if let Some(output) = decide(&root, db, &hook, args)? {
        println!("{output}");
    }
    Ok(())
}

#[derive(Serialize)]
struct SkillPage {
    path: PathBuf,
    rel: String,
    title: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Serialize)]
struct SkillPages {
    pages: Vec<SkillPage>,
}

fn skill_pages(root: &Path, db: &Connection, args: &SkillPagesArgs) -> Result<SkillPages> {
    let names = skill_names(&args.skill);
    if names.is_empty() || args.n == 0 {
        return Ok(SkillPages { pages: vec![] });
    }
    let root = root.canonicalize()?;
    let project = crate::context::current_project(&args.cwd);
    let seen = if let Some(session) = &args.session {
        session_seen(db, session)?
    } else {
        HashSet::new()
    };
    let pages: Vec<_> = skill_candidates(db, &names, project.as_deref())?
        .into_iter()
        .filter(|page| !seen.contains(&page.path))
        .take(args.n)
        .map(|page| SkillPage {
            path: root.join(&page.path),
            kind: crate::pages::page_type(&page.path).into(),
            rel: page.path,
            title: page.title,
        })
        .collect();
    if let Some(session) = &args.session {
        let paths: Vec<_> = pages.iter().map(|page| page.rel.clone()).collect();
        mark_seen(db, session, &paths)?;
        if let Some(source) = &args.source {
            crate::usage::log(&root, session, source, &paths);
        }
    }
    Ok(SkillPages { pages })
}

pub fn run_skill_pages(root: &Path, db: &Connection, args: &SkillPagesArgs) -> Result<()> {
    let report = skill_pages(root, db, args)?;
    if args.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for page in report.pages {
            println!("{} ({})", page.path.display(), page.title);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };

    const ERROR: &str =
        "error building bubblewrap command: cannot establish app-server socket mount isolation";

    fn hook(event: &str, tool: &str, input: Value) -> Hook {
        Hook {
            hook_event_name: event.into(),
            tool_name: tool.into(),
            tool_input: input,
            ..Hook::default()
        }
    }

    fn failure(error: Value) -> Hook {
        Hook {
            error,
            ..hook("PostToolUseFailure", "Bash", json!({"command":"unrelated"}))
        }
    }

    fn args() -> Args {
        Args {
            min_windows: 2,
            max_skill_pages: 5,
            max_pages: 2,
            include_history: false,
            explain: false,
        }
    }

    fn skill_args(cwd: &Path) -> SkillPagesArgs {
        SkillPagesArgs {
            skill: "vendor:demo-skill".into(),
            json: true,
            n: 5,
            cwd: cwd.into(),
            session: None,
            source: None,
        }
    }

    #[test]
    fn skill_pages_filters_orders_limits_and_serializes() -> Result<()> {
        let mut wiki = crate::usage::tests::Wiki::new()?;
        let project = crate::context::current_project(&wiki.root).unwrap();
        for (rel, tag, scope, status) in [
            ("knowledge/project", "DEMO-SKILL", project.as_str(), ""),
            ("knowledge/global", "demo-skill", "", ""),
            ("knowledge/full", "vendor:demo-skill", "", ""),
            ("followups/open", "demo-skill", project.as_str(), "open"),
            ("followups/closed", "demo-skill", "", "done"),
            ("followups/missing-status", "demo-skill", "", ""),
            ("knowledge/other", "demo-skill", "elsewhere", ""),
            ("knowledge/partial", "demo-skill-extra", "", ""),
            ("knowledge/blocked", "demo-skill", "", ""),
            ("projects/overview", "demo-skill", "", ""),
            ("history/past", "demo-skill", "", ""),
        ] {
            wiki.page(&format!("pages/{rel}.md"), &format!("---\ntitle: {rel}\ntags: [{tag}]\nproject: '{scope}'\nstatus: '{status}'\ncreated: 2020-01-01\n---\nbody"))?;
        }
        wiki.index.execute_batch(
            "update files set mtime_ns=100;
            update files set mtime_ns=900 where path='pages/knowledge/global.md';
            update files set mtime_ns=200 where path='pages/knowledge/project.md';
            update files set blocked='secret' where path='pages/knowledge/blocked.md';",
        )?;
        let mut args = skill_args(&wiki.root);
        let report = skill_pages(&wiki.root, &wiki.index, &args)?;
        assert_eq!(
            report
                .pages
                .iter()
                .map(|page| page.rel.as_str())
                .collect::<Vec<_>>(),
            [
                "pages/knowledge/project.md",
                "pages/followups/open.md",
                "pages/knowledge/global.md",
                "pages/knowledge/full.md"
            ]
        );
        args.n = 2;
        assert_eq!(
            serde_json::to_value(skill_pages(&wiki.root, &wiki.index, &args)?)?,
            json!({"pages": [
                {"path": wiki.root.join("pages/knowledge/project.md"), "rel": "pages/knowledge/project.md", "title": "knowledge/project", "type": "knowledge"},
                {"path": wiki.root.join("pages/followups/open.md"), "rel": "pages/followups/open.md", "title": "followups/open", "type": "followups"}
            ]})
        );
        args.cwd = PathBuf::from("/");
        assert_eq!(
            skill_pages(&wiki.root, &wiki.index, &args)?
                .pages
                .iter()
                .map(|page| page.rel.as_str())
                .collect::<Vec<_>>(),
            ["pages/knowledge/global.md", "pages/knowledge/full.md"]
        );
        for skill in ["claude-wiki", "plugin:CLAUDE-WIKI", "unmatched"] {
            args.skill = skill.into();
            assert_eq!(
                serde_json::to_value(skill_pages(&wiki.root, &wiki.index, &args)?)?,
                json!({"pages": []})
            );
        }
        assert!(session_seen(&wiki.index, "")?.is_empty());
        assert!(!wiki.root.join("usage.sqlite").exists());
        Ok(())
    }

    #[test]
    fn skill_pages_session_dedupes_before_limit_and_logging_requires_both_options() -> Result<()> {
        let wiki = Wiki::new()?;
        wiki.page("a", "demo-skill", "", "")?;
        wiki.page("b", "demo-skill", "", "")?;
        let mut args = skill_args(&wiki.root);
        args.n = 1;
        args.source = Some("mod-skill".into());
        for _ in 0..2 {
            assert_eq!(
                skill_pages(&wiki.root, &wiki.db, &args)?.pages[0].rel,
                "pages/knowledge/a.md"
            );
        }
        assert!(session_seen(&wiki.db, "")?.is_empty());
        assert!(!wiki.root.join("usage.sqlite").exists());
        args.session = Some("s".into());
        args.source = None;
        assert_eq!(
            skill_pages(&wiki.root, &wiki.db, &args)?.pages[0].rel,
            "pages/knowledge/a.md"
        );
        assert!(!wiki.root.join("usage.sqlite").exists());
        args.source = Some("mod-skill".into());
        assert_eq!(
            skill_pages(&wiki.root, &wiki.db, &args)?.pages[0].rel,
            "pages/knowledge/b.md"
        );
        assert!(skill_pages(&wiki.root, &wiki.db, &args)?.pages.is_empty());
        assert_eq!(session_seen(&wiki.db, "s")?.len(), 2);
        let usage = crate::usage::open(&wiki.root)?;
        let rows = usage
            .prepare("select session_id, path, source from injections")?
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(
            rows,
            [(
                "s".into(),
                "pages/knowledge/b.md".into(),
                "mod-skill".into()
            )]
        );
        args.session = Some("other".into());
        args.n = 0;
        assert!(skill_pages(&wiki.root, &wiki.db, &args)?.pages.is_empty());
        assert!(session_seen(&wiki.db, "other")?.is_empty());
        args.n = 5;
        assert_eq!(skill_pages(&wiki.root, &wiki.db, &args)?.pages.len(), 2);
        Ok(())
    }

    #[test]
    fn skill_pages_cli_defaults_and_source_validation() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(["claude-wiki", "skill-pages", "demo-skill"]).unwrap();
        let crate::Commands::SkillPages(args) = cli.command else {
            panic!("wrong command")
        };
        assert_eq!(args.n, 5);
        assert_eq!(args.cwd, Path::new("."));
        assert_eq!(args.skill, "demo-skill");
        for source in ["Bad", "mod_skill", "a1", "", &"a".repeat(32)] {
            assert_eq!(
                crate::Cli::try_parse_from([
                    "claude-wiki",
                    "skill-pages",
                    "demo-skill",
                    "--source",
                    source
                ])
                .err()
                .unwrap()
                .exit_code(),
                2
            );
        }
    }

    struct Wiki {
        root: PathBuf,
        db: Connection,
    }
    impl Wiki {
        fn new() -> Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "cw-recall-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root)?;
            let db = crate::index::open(&root)?;
            Ok(Self { root, db })
        }

        fn page(&self, name: &str, tags: &str, project: &str, body: &str) -> Result<()> {
            let path = format!("pages/knowledge/{name}.md");
            self.db
                .execute("insert into files(path, mtime_ns) values (?, 100)", [&path])?;
            self.db.execute("insert into fts(rowid, path, type, status, title, tags, project, body) values (?, ?, 'knowledge', '', ?, ?, ?, ?)",
                params![self.db.last_insert_rowid(), path, name, tags, project, body])?;
            Ok(())
        }

        fn explain(&self, h: &Hook) -> Result<Value> {
            let mut opts = args();
            opts.explain = true;
            Ok(decide(&self.root, &self.db, h, &opts)?.unwrap())
        }
    }
    impl Drop for Wiki {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn wiki_paths_typed_with_home_are_wiki_operations() {
        let root = crate::home().join(".local/share/claude-wiki");
        for command in [
            "cd ~/.local/share/claude-wiki/pages && sed -i 1d knowledge/x.md",
            "cat $HOME/.local/share/claude-wiki/pages/knowledge/x.md",
            "ls ${HOME}/.local/share/claude-wiki",
        ] {
            let h = hook("PostToolUseFailure", "Bash", json!({ "command": command }));
            assert!(wiki_operation(&root, &h), "{command}");
        }
        let h = hook(
            "PostToolUseFailure",
            "Bash",
            json!({"command": "cat ~/notes.md"}),
        );
        assert!(!wiki_operation(&root, &h));
    }

    #[test]
    fn windows_preserve_short_words_and_reject_identifiers() {
        let home = Path::new("/home/example-user");
        let windows = error_windows("ERROR: it is not a valid option", home);
        assert_eq!(
            windows
                .iter()
                .map(|w| w.phrase.as_str())
                .collect::<Vec<_>>(),
            ["error it is not a valid", "it is not a valid option"]
        );
        for token in ["123", "１２３", "abc12def", "home", "example", "user"] {
            let windows = error_windows(&format!("error cannot open the {token} socket"), home);
            assert_eq!(windows.len(), 1, "{token}");
            assert_eq!(windows[0].phrase, "error cannot open the", "{token}");
            assert!(windows[0].short);
        }
        assert_eq!(
            error_windows("error cannot open the abcdefa socket", home).len(),
            1
        );
        assert!(error_windows("one two three four five six seven", home).is_empty());
        let windows = error_windows("error one two\nfatal three four", home);
        assert_eq!(
            windows
                .iter()
                .map(|w| w.phrase.as_str())
                .collect::<Vec<_>>(),
            ["error one two", "fatal three four"]
        );
        assert!(windows.iter().all(|w| w.short));
        assert_eq!(
            error_windows("오류가 error 한글과 두 글자 이상의 단어", home).len(),
            2
        );
    }

    #[test]
    fn segments_do_not_bridge_dropped_tokens() {
        let windows = error_windows(
            "error one two 123 three four five six abc12def seven eight nine ten eleven /home/test/ twelve thirteen fourteen fifteen sixteen seventeen eighteen",
            Path::new("/home/test"),
        );
        assert_eq!(
            windows
                .iter()
                .map(|w| (w.phrase.as_str(), w.short))
                .collect::<Vec<_>>(),
            [
                ("error one two", true),
                ("three four five six", true),
                ("seven eight nine ten eleven", true),
                ("twelve thirteen fourteen fifteen sixteen seventeen", false),
                (
                    "thirteen fourteen fifteen sixteen seventeen eighteen",
                    false
                ),
            ]
        );
        assert!(error_windows("error one 123 two three", Path::new("/home/test")).is_empty());
    }

    #[test]
    fn short_phrases_require_rare_tokens_and_score_two() -> Result<()> {
        let wiki = Wiki::new()?;
        wiki.page("ssh", "", "", "Permission denied (publickey).")?;
        wiki.page("common", "", "", "error connection refused")?;
        for name in ["noise-one", "noise-two", "noise-three", "noise-four"] {
            wiki.page(
                name,
                "",
                "",
                "permission error denied connection was refused; fatal shared quux",
            )?;
        }
        for (error, df, reason) in [
            ("Permission denied (publickey).", 1, None),
            ("error connection refused", 1, Some("no rare token")),
            ("fatal shared quux", 4, Some("shared by too many pages")),
            ("error missing diagnostic", 0, Some("no matches")),
        ] {
            let h = failure(json!(error));
            let output = wiki.explain(&h)?;
            let windows = output["windows"].as_array().unwrap();
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0]["short"], true);
            assert_eq!(windows[0]["df"], df);
            assert_eq!(windows[0]["reason"], json!(reason));
            assert_eq!(windows[0]["kept"], reason.is_none());
            if reason.is_none() {
                assert_eq!(output["selected"], json!(["pages/knowledge/ssh.md"]));
                assert_eq!(output["candidates"][0]["score"], 2);
                assert_eq!(output["candidates"][0]["qualified"], true);
                let mut opts = args();
                opts.min_windows = 3;
                assert!(decide(&wiki.root, &wiki.db, &h, &opts)?.is_none());
                assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_some());
            } else {
                assert!(output["selected"].as_array().unwrap().is_empty());
            }
        }
        Ok(())
    }

    #[test]
    fn only_last_lines_and_last_distinct_windows_are_used() {
        let home = Path::new("/home/test");
        assert_eq!(error_windows(&format!("{ERROR}\n{ERROR}"), home).len(), 6);
        let old = format!("{ERROR}\n{}", "ordinary output\n".repeat(200));
        assert!(error_windows(&old, home).is_empty());
        let lines = (0..100)
            .map(|i| format!("error cannot establish the socket token{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let windows = error_windows(&lines, home);
        assert_eq!(windows.len(), MAX_WINDOWS);
        assert!(windows[0].phrase.ends_with("token20"));
        assert!(windows.last().unwrap().phrase.ends_with("token99"));
    }

    #[test]
    fn skill_tags_project_scope_types_and_order() -> Result<()> {
        let wiki = Wiki::new()?;
        for (name, tags, project) in [
            ("project", "unrelated CODEX-IMPLEMENT", "demo"),
            ("global", "codex-implement", ""),
            ("open", "codex-implement", "demo"),
            ("closed", "codex-implement", "demo"),
            ("missing-status", "codex-implement", "demo"),
            ("other", "codex-implement", "elsewhere"),
            ("untagged", "", "demo"),
            ("partial", "codex-implement-extra", "demo"),
            ("overview", "codex-implement", "demo"),
            ("history", "codex-implement", "demo"),
            ("blocked", "codex-implement", "demo"),
        ] {
            wiki.page(name, tags, project, "")?;
        }
        wiki.db.execute_batch(
            "update fts set type = 'followups', status = 'open' where title = 'open';
            update fts set type = 'followups', status = 'done' where title = 'closed';
            update fts set type = 'followups' where title = 'missing-status';
            update fts set type = 'projects' where title = 'overview';
            update fts set type = 'history' where title = 'history';
            update files set blocked = 'secret' where path like '%blocked.md';
            update files set mtime_ns = 300 where path like '%global.md';
            update files set mtime_ns = 200 where path like '%open.md';",
        )?;
        let names = vec!["codex-implement".into()];
        let pages = skill_candidates(&wiki.db, &names, Some("demo"))?;
        assert_eq!(
            pages.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
            ["open", "project", "global"]
        );
        assert!(pages[0].project_match && pages[1].project_match && !pages[2].project_match);
        for project in [None, Some("absent")] {
            let pages = skill_candidates(&wiki.db, &names, project)?;
            assert_eq!(pages.len(), 1);
            assert_eq!(pages[0].title, "global");
        }
        Ok(())
    }

    #[test]
    fn plugin_skills_match_full_and_suffix_tags() -> Result<()> {
        let wiki = Wiki::new()?;
        wiki.page("full", "vendor:tools:codex-implement", "", "")?;
        wiki.page("suffix", "CoDeX-ImPlEmEnT", "", "")?;
        wiki.page("partial", "vendor:tools:codex-implement-other", "", "")?;
        let h = hook(
            "PreToolUse",
            "Skill",
            json!({"skill":"Vendor:Tools:Codex-Implement"}),
        );
        let output = wiki.explain(&h)?;
        assert_eq!(output["mode"], "skill");
        assert_eq!(
            output["skill_names"],
            json!(["vendor:tools:codex-implement", "codex-implement"])
        );
        assert_eq!(output["selected"].as_array().unwrap().len(), 2);
        for name in ["claude-wiki", "plugin:CLAUDE-WIKI", ""] {
            let h = hook("PreToolUse", "Skill", json!({"skill":name}));
            assert!(wiki.explain(&h)?["mode"].is_null());
            assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_none());
        }
        Ok(())
    }

    #[test]
    fn hook_cwd_resolves_main_project_even_from_worktrees() -> Result<()> {
        let wiki = Wiki::new()?;
        let repo = wiki.root.join("demo");
        fs::create_dir_all(repo.join("nested"))?;
        crate::git(&repo, &["init", "-q"])?;
        crate::git(
            &repo,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        )?;
        let worktree = wiki.root.join("different-name");
        crate::git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "recall-test",
                worktree.to_str().unwrap(),
            ],
        )?;
        wiki.page("matching", "tooling", "demo", "")?;
        wiki.page("other", "tooling", "different-name", "")?;
        wiki.page("global", "tooling", "", "")?;
        for cwd in [repo.join("nested"), worktree] {
            let mut h = hook("PreToolUse", "Skill", json!({"skill":"tooling"}));
            h.cwd = Some(cwd.to_string_lossy().into_owned());
            let output = wiki.explain(&h)?;
            assert_eq!(output["project"], "demo");
            assert_eq!(
                output["selected"],
                json!(["pages/knowledge/matching.md", "pages/knowledge/global.md"])
            );
            let mut args = skill_args(&cwd);
            args.skill = "tooling".into();
            let pages = skill_pages(&wiki.root, &wiki.db, &args)?;
            assert_eq!(
                json!(pages.pages.iter().map(|page| &page.rel).collect::<Vec<_>>()),
                output["selected"]
            );
        }
        for cwd in [None, Some(wiki.root.to_string_lossy().into_owned())] {
            let mut h = hook("PreToolUse", "Skill", json!({"skill":"tooling"}));
            h.cwd = cwd;
            let output = wiki.explain(&h)?;
            assert!(output["project"].is_null());
            assert_eq!(output["selected"], json!(["pages/knowledge/global.md"]));
        }
        Ok(())
    }

    #[test]
    fn exact_error_windows_score_and_sort_by_recency() -> Result<()> {
        let wiki = Wiki::new()?;
        wiki.page("old", "", "", ERROR)?;
        wiki.page("new", "", "", ERROR)?;
        wiki.page(
            "one-window",
            "",
            "",
            "error building bubblewrap command cannot establish",
        )?;
        wiki.page("blocked", "", "", ERROR)?;
        wiki.page("history", "", "", ERROR)?;
        wiki.db.execute_batch(
            "update files set mtime_ns = 200 where path like '%new.md';
            update files set blocked = 'secret' where path like '%blocked.md';
            update fts set type = 'history' where title = 'history';",
        )?;
        let h = failure(json!(ERROR));
        let output = wiki.explain(&h)?;
        assert_eq!(output["mode"], "error");
        assert!(
            output["windows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["short"] == false)
        );
        assert_eq!(
            output["selected"],
            json!(["pages/knowledge/new.md", "pages/knowledge/old.md"])
        );
        let candidates = output["candidates"].as_array().unwrap();
        assert_eq!(candidates[0]["score"], 6);
        assert_eq!(candidates[2]["score"], 1);
        assert_eq!(candidates[2]["qualified"], false);
        let mut opts = args();
        opts.include_history = true;
        opts.explain = true;
        let output = decide(&wiki.root, &wiki.db, &h, &opts)?.unwrap();
        assert_eq!(output["windows"][0]["df"], 4);
        assert_eq!(output["windows"][0]["kept"], false);
        assert!(
            output["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["title"] == "history")
        );
        Ok(())
    }

    #[test]
    fn single_or_common_windows_do_not_qualify() -> Result<()> {
        let wiki = Wiki::new()?;
        wiki.page("one", "", "", ERROR)?;
        let h = failure(json!("error building bubblewrap command cannot establish"));
        assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_none());
        let mut opts = args();
        opts.min_windows = 1;
        assert!(decide(&wiki.root, &wiki.db, &h, &opts)?.is_some());
        for name in ["two", "three", "four"] {
            wiki.page(name, "", "", ERROR)?;
        }
        let output = wiki.explain(&failure(json!(ERROR)))?;
        assert!(output["selected"].as_array().unwrap().is_empty());
        assert!(output["candidates"].as_array().unwrap().is_empty());
        assert!(
            output["windows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w["df"] == 4 && w["kept"] == false && w["reason"].is_string())
        );
        Ok(())
    }

    #[test]
    fn ignored_calls_and_failures_never_use_tool_input() -> Result<()> {
        let wiki = Wiki::new()?;
        wiki.page("target", "tooling", "", ERROR)?;
        for (event, tool) in [
            ("PreToolUse", "Bash"),
            ("PreToolUse", "Edit"),
            ("PostToolUse", "Bash"),
            ("unknown", "Skill"),
        ] {
            let mut h = hook(
                event,
                tool,
                json!({"skill":"tooling", "command":ERROR, "file_path":ERROR}),
            );
            h.error = json!(ERROR);
            assert!(wiki.explain(&h)?["mode"].is_null());
            assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_none());
        }
        for error in [
            Value::Null,
            json!(3),
            json!({"message":ERROR}),
            json!("unrelated error"),
        ] {
            let mut h = failure(error);
            h.tool_input = json!({"command":ERROR});
            assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_none());
        }
        let mut h = failure(json!(ERROR));
        h.is_interrupt = true;
        assert!(wiki.explain(&h)?["mode"].is_null());
        assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_none());
        h.is_interrupt = false;
        h.tool_name = "Read".into();
        assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_some());
        Ok(())
    }

    #[test]
    fn wiki_operations_and_fts_syntax_are_not_queries() -> Result<()> {
        let wiki = Wiki::new()?;
        wiki.page("target", "", "", ERROR)?;
        for command in [
            "claude-wiki search clipboard",
            " \tclaude-wiki",
            "cd /repo && claude-wiki sync",
            "FOO=bar BAZ='two words' claude-wiki search",
            "cd '/repo path' && FOO=\"a b\" claude-wiki sync",
            "cat /wiki/pages/knowledge/foo.md",
        ] {
            let mut h = failure(json!(ERROR));
            h.tool_input = json!({"command":command});
            assert!(
                decide(Path::new("/wiki"), &wiki.db, &h, &args())?.is_none(),
                "{command}"
            );
        }
        let mut h = failure(json!(ERROR));
        h.tool_name = "Edit".into();
        h.cwd = Some("/wiki".into());
        h.tool_input = json!({"file_path":"./pages/knowledge/foo.md"});
        assert!(wiki_operation(Path::new("/wiki"), &h));
        assert_eq!(quote("alpha\" OR beta"), "\"alpha\"\" OR beta\"");
        let n: i64 = wiki.db.query_row(
            "select count(*) from fts where fts match ?",
            [quote("error\" OR bubblewrap")],
            |r| r.get(0),
        )?;
        assert_eq!(n, 0);
        Ok(())
    }

    #[test]
    fn limits_session_dedupe_and_explain_do_not_access_state() -> Result<()> {
        let wiki = Wiki::new()?;
        for name in ["one", "two", "three", "four", "five", "six"] {
            wiki.page(name, "tooling", "", "")?;
        }
        let h = hook("PreToolUse", "Skill", json!({"skill":"tooling"}));
        let output = decide(&wiki.root, &wiki.db, &h, &args())?.unwrap();
        let message = output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(message.starts_with("claude-wiki: wiki pages about the tooling skill:"));
        assert_eq!(message.matches(".md (").count(), 5);
        assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_some());
        assert!(decide(&wiki.root, &wiki.db, &h, &args())?.is_none());
        let mut other = Hook {
            session_id: Some("other".into()),
            ..h
        };
        let mut opts = args();
        opts.max_skill_pages = 1;
        let output = decide(&wiki.root, &wiki.db, &other, &opts)?.unwrap();
        assert_eq!(
            output["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .matches(".md (")
                .count(),
            1
        );
        wiki.page("failure", "", "", ERROR)?;
        other = failure(json!(ERROR));
        assert!(decide(&wiki.root, &wiki.db, &other, &args())?.is_some());
        assert!(decide(&wiki.root, &wiki.db, &other, &args())?.is_none());
        // Any accidental state read or write now fails both explain modes.
        wiki.db
            .execute_batch("drop table recall_seen; drop table recall_state;")?;
        for h in [
            other,
            hook("PreToolUse", "Skill", json!({"skill":"tooling"})),
        ] {
            let output = wiki.explain(&h)?;
            assert!(!output["selected"].as_array().unwrap().is_empty());
            assert!(
                output["candidates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|c| c["seen"] == false)
            );
        }
        Ok(())
    }

    #[test]
    fn transcript_complete_lines_shrink_and_reminder_paths() -> Result<()> {
        let wiki = Wiki::new()?;
        for name in ["one", "two", "three", "four"] {
            wiki.page(name, "", "", "")?;
        }
        let path = wiki.root.join("transcript.jsonl");
        let mut h = Hook {
            transcript_path: Some(path.to_string_lossy().into_owned()),
            ..Hook::default()
        };
        assert!(
            transcript_seen(
                &wiki.db,
                h.session_id.as_deref().unwrap_or_default(),
                h.transcript_path.as_deref()
            )?
            .is_empty()
        );
        let line = |role: &str, part: &str, input: Value| {
            json!({"type":role, "message":{"content":[{"type":part, "input":input}]}}).to_string()
        };
        let ignored = line("user", "tool_use", json!("pages/knowledge/three.md"));
        let prose = line("assistant", "text", json!("pages/knowledge/three.md"));
        let one = line(
            "assistant",
            "tool_use",
            json!({"command":"sed -n '1,10p' pages/knowledge/one.md"}),
        );
        let two = line(
            "assistant",
            "tool_use",
            json!({"file_path":wiki.root.join("pages/knowledge/two.md")}),
        );
        fs::write(&path, format!("bad json\n{ignored}\n{prose}\n{one}\n{two}"))?;
        let seen = transcript_seen(
            &wiki.db,
            h.session_id.as_deref().unwrap_or_default(),
            h.transcript_path.as_deref(),
        )?;
        assert_eq!(seen, HashSet::from(["pages/knowledge/one.md".into()]));
        fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"\n")?;
        assert_eq!(
            transcript_seen(
                &wiki.db,
                h.session_id.as_deref().unwrap_or_default(),
                h.transcript_path.as_deref()
            )?
            .len(),
            2
        );
        let three = line(
            "assistant",
            "tool_use",
            json!({"command":"cat pages/knowledge/three.md"}),
        );
        fs::write(&path, format!("{three}\n"))?;
        assert_eq!(
            transcript_seen(
                &wiki.db,
                h.session_id.as_deref().unwrap_or_default(),
                h.transcript_path.as_deref()
            )?
            .len(),
            3
        );
        mark_seen(&wiki.db, "", &["knowledge/four".into()])?;
        assert_eq!(
            transcript_seen(
                &wiki.db,
                h.session_id.as_deref().unwrap_or_default(),
                h.transcript_path.as_deref()
            )?
            .len(),
            4
        );
        h.session_id = Some("other".into());
        assert_eq!(
            transcript_seen(
                &wiki.db,
                h.session_id.as_deref().unwrap_or_default(),
                h.transcript_path.as_deref()
            )?
            .len(),
            1
        );
        Ok(())
    }
}

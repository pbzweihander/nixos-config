//! Shared bulletin board: pages tagged `bulletin` are announced once per change to
//! every consumer (an Orca bot, or otherwise a session) at turn or session start.
//!
//! A page counts as changed when its indexed `files.mtime_ns` differs from the value
//! the consumer last saw. Seen state lives in git-ignored `usage.sqlite`, so a rebuilt
//! page index does not re-announce anything that did not change on disk.
use anyhow::{Result, bail};
use chrono::{Local, TimeZone, Utc};
use clap::Args as ClapArgs;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

pub const TAG: &str = "bulletin";

#[derive(ClapArgs)]
pub struct Args {
    /// Consumer key (`bot:<id>` or `session:<id>`); derived from --cwd when omitted
    #[arg(long)]
    pub consumer: Option<String>,
    /// Directory used to detect an Orca bot home (`.../.orca/bots/<id>`); defaults to the current directory
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// Mark every listed page as seen by the consumer
    #[arg(long)]
    pub mark_seen: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub path: String,
    pub title: String,
    pub mtime: i64,
    pub seen: Option<i64>,
}

impl Notice {
    pub fn unseen(&self) -> bool {
        // `!=` rather than `>`: an mtime that moves backwards (restored file, clock
        // skew between hosts sharing the wiki) is still a change worth announcing.
        self.seen != Some(self.mtime)
    }
    fn status(&self, known: bool) -> &'static str {
        match (known, self.seen) {
            (false, _) => "unknown",
            (true, None) => "new",
            (true, Some(_)) if self.unseen() => "updated",
            _ => "seen",
        }
    }
}

/// `bot-3` for any path with the components `.orca/bots/bot-3`.
pub fn bot_id(path: &Path) -> Option<String> {
    let parts: Vec<&OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    parts.windows(3).find_map(|w| {
        (w[0] == ".orca" && w[1] == "bots" && !w[2].is_empty())
            .then(|| w[2].to_string_lossy().into_owned())
    })
}

fn bot_in(path: &Path) -> Option<String> {
    bot_id(path).or_else(|| bot_id(&path.canonicalize().ok()?))
}

/// Orca bot first (`$ORCA_BOT_HOME`, then the working directory), else the session.
pub fn consumer_key(bot_home: Option<&Path>, cwd: Option<&Path>, session: &str) -> Option<String> {
    bot_home
        .filter(|p| !p.as_os_str().is_empty())
        .and_then(bot_in)
        .or_else(|| cwd.and_then(bot_in))
        .map(|id| format!("bot:{id}"))
        .or_else(|| (!session.is_empty()).then(|| format!("session:{session}")))
}

fn env_key(cwd: Option<&Path>, session: &str) -> Option<String> {
    let bot_home = std::env::var_os("ORCA_BOT_HOME").map(PathBuf::from);
    consumer_key(bot_home.as_deref(), cwd, session)
}

pub fn open_seen(root: &Path) -> Result<Connection> {
    let db = crate::usage::open(root)?;
    db.execute_batch(
        "create table if not exists bulletin_seen(
            consumer_key text not null,
            page_path text not null,
            seen_mtime integer not null,
            updated_at integer not null,
            primary key(consumer_key, page_path));",
    )?;
    Ok(db)
}

/// Unblocked `bulletin` pages, newest first, with the consumer's seen mtime if any.
pub fn notices(index: &Connection, seen: Option<(&Connection, &str)>) -> Result<Vec<Notice>> {
    let mut notices = index
        .prepare(
            "select fts.path, fts.title, files.mtime_ns from fts join files on files.id = fts.rowid
            where files.blocked = '' and (' ' || fts.tags || ' ') like ?
            order by files.mtime_ns desc, fts.path",
        )?
        .query_map([format!("% {TAG} %")], |r| {
            Ok(Notice {
                path: r.get(0)?,
                title: r.get(1)?,
                mtime: r.get(2)?,
                seen: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if let Some((db, key)) = seen {
        let mut stmt = db.prepare(
            "select seen_mtime from bulletin_seen where consumer_key = ? and page_path = ?",
        )?;
        for notice in &mut notices {
            notice.seen = stmt
                .query_row(params![key, notice.path], |r| r.get(0))
                .optional()?;
        }
    }
    Ok(notices)
}

pub fn mark_seen(db: &Connection, key: &str, notices: &[Notice]) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    let now = Utc::now().timestamp();
    for notice in notices {
        tx.execute(
            "insert into bulletin_seen(consumer_key, page_path, seen_mtime, updated_at) values (?,?,?,?)
            on conflict(consumer_key, page_path) do update set seen_mtime = excluded.seen_mtime, updated_at = excluded.updated_at",
            params![key, notice.path, notice.mtime, now],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn updated(mtime: i64) -> String {
    Local
        .timestamp_opt(
            mtime.div_euclid(1_000_000_000),
            u32::try_from(mtime.rem_euclid(1_000_000_000)).unwrap_or(0),
        )
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

pub fn message(root: &Path, notices: &[Notice]) -> String {
    let pages = notices
        .iter()
        .map(|n| {
            format!(
                "{} ({} — updated {})",
                root.join(&n.path).display(),
                n.title.chars().take(120).collect::<String>(),
                updated(n.mtime)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "agent-wiki bulletin: updated notice(s) since your last check: {pages}. Read and apply any changes that affect your work or SOP; do not reply to this note directly."
    )
}

/// Turn/session-start check for hooks: returns the note and marks the pages seen.
/// Without a hook payload (`cwd` and `session` both empty) it does nothing. Errors
/// are reported on stderr and never suppress the rest of the hook output.
pub fn hook(root: &Path, index: &Connection, cwd: Option<&str>, session: &str) -> Option<String> {
    let cwd = match cwd.filter(|s| !s.is_empty()) {
        Some(cwd) => Some(PathBuf::from(cwd)),
        None if session.is_empty() => return None,
        None => std::env::current_dir().ok(),
    };
    let key = env_key(cwd.as_deref(), session)?;
    let result = (|| -> Result<Option<String>> {
        if notices(index, None)?.is_empty() {
            return Ok(None);
        }
        let db = open_seen(root)?;
        let unseen: Vec<_> = notices(index, Some((&db, &key)))?
            .into_iter()
            .filter(Notice::unseen)
            .collect();
        if unseen.is_empty() {
            return Ok(None);
        }
        let root = root.canonicalize()?;
        let text = message(&root, &unseen);
        mark_seen(&db, &key, &unseen)?;
        let paths: Vec<_> = unseen.iter().map(|n| n.path.clone()).collect();
        crate::usage::log(&root, session, "bulletin", &paths);
        Ok(Some(text))
    })();
    result.unwrap_or_else(|e| {
        eprintln!("agent-wiki: bulletin: {e:#}");
        None
    })
}

#[derive(Serialize)]
struct PageJson<'a> {
    path: &'a str,
    abs_path: PathBuf,
    title: &'a str,
    mtime_ns: i64,
    updated: String,
    seen_mtime_ns: Option<i64>,
    status: &'static str,
}

#[derive(Serialize)]
struct Report<'a> {
    consumer: Option<&'a str>,
    marked_seen: usize,
    pages: Vec<PageJson<'a>>,
}

pub fn run(root: &Path, index: &Connection, args: &Args) -> Result<()> {
    let cwd = match &args.cwd {
        Some(cwd) => Some(cwd.clone()),
        None => std::env::current_dir().ok(),
    };
    let key = args
        .consumer
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| env_key(cwd.as_deref(), ""));
    if args.mark_seen && key.is_none() {
        bail!(
            "no consumer: pass --consumer bot:<id> or session:<id>, or run inside an Orca bot home"
        );
    }
    let db = key.as_ref().map(|_| open_seen(root)).transpose()?;
    let notices = notices(index, db.as_ref().zip(key.as_deref()))?;
    let marked = match (&db, &key) {
        (Some(db), Some(key)) if args.mark_seen => {
            let unseen: Vec<_> = notices.iter().filter(|n| n.unseen()).cloned().collect();
            mark_seen(db, key, &unseen)?;
            unseen.len()
        }
        _ => 0,
    };
    let root = root.canonicalize()?;
    if args.json {
        let pages = notices
            .iter()
            .map(|n| PageJson {
                path: &n.path,
                abs_path: root.join(&n.path),
                title: &n.title,
                mtime_ns: n.mtime,
                updated: updated(n.mtime),
                seen_mtime_ns: n.seen,
                status: n.status(key.is_some()),
            })
            .collect();
        let report = Report {
            consumer: key.as_deref(),
            marked_seen: marked,
            pages,
        };
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    println!(
        "consumer: {}",
        key.as_deref()
            .unwrap_or("(none; pass --consumer to see seen state)")
    );
    if notices.is_empty() {
        println!("no pages tagged `{TAG}`");
    }
    for n in &notices {
        println!(
            "{:<8} {} ({} — updated {})",
            n.status(key.is_some()),
            root.join(&n.path).display(),
            n.title,
            updated(n.mtime)
        );
    }
    if args.mark_seen {
        println!("marked {marked} page(s) seen");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::tests::Wiki;
    use std::{
        fs::File,
        time::{Duration, SystemTime},
    };

    const BULLETIN: &str =
        "---\ntitle: Team notice\ncreated: 2026-11-26\ntags: [orca, bulletin]\n---\nbody\n";

    fn touch(wiki: &mut Wiki, rel: &str, secs: u64) -> Result<()> {
        File::options()
            .write(true)
            .open(wiki.root.join(rel))?
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))?;
        crate::index::refresh(&wiki.root, &mut wiki.index)
    }

    fn check(wiki: &Wiki, session: &str) -> Option<String> {
        hook(&wiki.root, &wiki.index, Some("/srv/project"), session)
    }

    #[test]
    fn consumer_key_prefers_bot_home_then_session() {
        let bot = Path::new("/home/u/.orca/bots/bot-3/workspace/sub");
        assert_eq!(
            consumer_key(None, Some(bot), "s1").as_deref(),
            Some("bot:bot-3")
        );
        assert_eq!(
            consumer_key(
                Some(Path::new("/home/u/.orca/bots/bot-2")),
                Some(Path::new("/srv/x")),
                "s1"
            )
            .as_deref(),
            Some("bot:bot-2")
        );
        assert_eq!(
            consumer_key(
                Some(Path::new("")),
                Some(Path::new("/home/u/nixos-config")),
                "s1"
            )
            .as_deref(),
            Some("session:s1")
        );
        // A directory merely named like a bot is not a bot home.
        assert_eq!(
            consumer_key(None, Some(Path::new("/home/u/orca/bots/bot-3")), "s1").as_deref(),
            Some("session:s1")
        );
        assert_eq!(
            consumer_key(None, Some(Path::new("/home/u/.orca/bots")), ""),
            None
        );
        assert_eq!(consumer_key(None, None, ""), None);
    }

    #[test]
    fn new_page_is_announced_once_then_again_after_an_edit() -> Result<()> {
        let mut wiki = Wiki::new()?;
        wiki.page(
            "pages/knowledge/plain.md",
            "---\ntitle: Plain\ncreated: 2026-11-26\ntags: [orca]\n---\nbody\n",
        )?;
        assert_eq!(check(&wiki, "s1"), None);
        wiki.page("pages/knowledge/notice.md", BULLETIN)?;
        touch(&mut wiki, "pages/knowledge/notice.md", 1_000)?;
        let note = check(&wiki, "s1").expect("new bulletin is announced");
        assert!(
            note.starts_with("agent-wiki bulletin: updated notice(s) since your last check: /")
        );
        assert!(note.contains("pages/knowledge/notice.md (Team notice — updated "));
        assert!(!note.contains("plain.md"));
        // seen_mtime == page.mtime: silent, also after an index rebuild.
        assert_eq!(check(&wiki, "s1"), None);
        crate::index::remove_index(&wiki.root, "index.sqlite")?;
        wiki.index = crate::index::open(&wiki.root)?;
        crate::index::refresh(&wiki.root, &mut wiki.index)?;
        assert_eq!(check(&wiki, "s1"), None);
        // Another consumer still gets it.
        assert!(check(&wiki, "s2").is_some());
        // An edit re-triggers it.
        touch(&mut wiki, "pages/knowledge/notice.md", 2_000)?;
        assert!(check(&wiki, "s1").is_some());
        assert_eq!(check(&wiki, "s1"), None);
        let db = open_seen(&wiki.root)?;
        let seen: i64 = db.query_row(
            "select seen_mtime from bulletin_seen where consumer_key='session:s1'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(seen, 2_000_000_000_000);
        let logged: i64 = db.query_row(
            "select count(*) from injections where source='bulletin' and session_id='s1'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(logged, 2);
        Ok(())
    }

    #[test]
    fn bot_consumers_share_state_across_sessions() -> Result<()> {
        let mut wiki = Wiki::new()?;
        wiki.page("pages/knowledge/notice.md", BULLETIN)?;
        let bot = Some("/home/u/.orca/bots/bot-3");
        assert!(hook(&wiki.root, &wiki.index, bot, "first").is_some());
        assert_eq!(hook(&wiki.root, &wiki.index, bot, "second"), None);
        assert!(hook(&wiki.root, &wiki.index, Some("/srv/p"), "second").is_some());
        // No hook payload at all: no state change.
        assert_eq!(hook(&wiki.root, &wiki.index, None, ""), None);
        Ok(())
    }

    #[test]
    fn blocked_bulletin_pages_are_never_announced() -> Result<()> {
        let mut wiki = Wiki::new()?;
        wiki.page("pages/knowledge/notice.md", BULLETIN)?;
        wiki.index.execute(
            "update files set blocked='secret' where path='pages/knowledge/notice.md'",
            [],
        )?;
        assert_eq!(check(&wiki, "s1"), None);
        assert!(notices(&wiki.index, None)?.is_empty());
        Ok(())
    }

    #[test]
    fn tag_match_is_whole_word() -> Result<()> {
        let mut wiki = Wiki::new()?;
        wiki.page(
            "pages/knowledge/a.md",
            "---\ntitle: A\ncreated: 2026-11-26\ntags: [bulletin-board]\n---\nx\n",
        )?;
        wiki.page(
            "pages/knowledge/b.md",
            "---\ntitle: B\ncreated: 2026-11-26\ntags: bulletin\n---\nx\n",
        )?;
        let paths: Vec<_> = notices(&wiki.index, None)?
            .into_iter()
            .map(|n| n.path)
            .collect();
        assert_eq!(paths, ["pages/knowledge/b.md"]);
        Ok(())
    }

    #[test]
    fn status_labels() {
        let mut n = Notice {
            path: "p".into(),
            title: "t".into(),
            mtime: 5,
            seen: None,
        };
        assert_eq!(n.status(false), "unknown");
        assert_eq!(n.status(true), "new");
        n.seen = Some(4);
        assert_eq!(n.status(true), "updated");
        n.seen = Some(5);
        assert_eq!(n.status(true), "seen");
        n.seen = Some(6);
        assert!(n.unseen());
    }
}

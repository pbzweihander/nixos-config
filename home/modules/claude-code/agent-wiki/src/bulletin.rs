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

/// `bot-3` for a resolved path with the components `.orca/bots/bot-3`. Callers must
/// pass a path from [`resolve`], which has no `.`/`..` components and no symlinks.
fn bot_id(path: &Path) -> Option<String> {
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

/// Absolute path with symlinks resolved and `.`/`..` applied the way the kernel
/// would: each existing prefix is canonicalized before the next component is read,
/// and below the first missing component the rest is normalized logically (a missing
/// directory cannot be a symlink). `..` pops a segment and stays at `/` at the root.
pub fn resolve(path: &Path) -> Option<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    if let Ok(path) = path.canonicalize() {
        return Some(path);
    }
    let mut out = PathBuf::from("/");
    let mut exists = true;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => {
                out.push(name);
                if exists {
                    match out.canonicalize() {
                        Ok(real) => out = real,
                        Err(_) => exists = false,
                    }
                }
            }
        }
        // After `..` leaves the missing part, the remaining prefix exists again.
        if !exists && out.exists() {
            exists = true;
            out = out.canonicalize().ok()?;
        }
    }
    Some(out)
}

fn bot_in(path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return None;
    }
    bot_id(&resolve(path)?)
}

/// `$ORCA_BOT_HOME`, when set and non-empty, decides alone whether this is a bot (the
/// working directory of a bot process may be any worktree); otherwise the working
/// directory does. Without a bot, the session is the consumer.
pub fn consumer_key(bot_home: Option<&Path>, cwd: Option<&Path>, session: &str) -> Option<String> {
    let bot = match bot_home.filter(|p| !p.as_os_str().is_empty()) {
        Some(home) => bot_in(home),
        None => cwd.and_then(bot_in),
    };
    bot.map(|id| format!("bot:{id}"))
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

/// Most pages announced in one note; the rest wait for the next turn.
pub const MAX_PAGES: usize = 10;
/// Character budget of the `remind` note. `context` uses a fifth of its own budget,
/// capped at this.
pub const MAX_CHARS: usize = 1500;
const TITLE_CHARS: usize = 120;
const HEAD: &str = "agent-wiki bulletin: updated notice(s) since your last check: ";
const TAIL: &str = ". Read and apply any changes that affect your work or SOP; do not reply to this note directly.";

fn more(n: usize) -> String {
    format!(" +{n} more (run `agent-wiki bulletin`)")
}

pub struct Note {
    pub text: String,
    /// Leading pages of the input that the text names; only these count as delivered.
    pub delivered: usize,
}

/// Packs as many leading notices as fit into `budget` characters (and at most
/// [`MAX_PAGES`]), shortening or dropping a title before giving up on a page, and
/// appends `+N more` for the rest. `None` when not even one page fits.
pub fn message(root: &Path, notices: &[Notice], budget: usize) -> Option<Note> {
    let fixed = HEAD.chars().count() + TAIL.chars().count();
    let mut text = String::new();
    let mut used = fixed;
    let mut delivered = 0;
    for (i, n) in notices.iter().enumerate().take(MAX_PAGES) {
        let rest = notices.len() - i - 1;
        let reserve = if rest > 0 {
            more(rest).chars().count()
        } else {
            0
        };
        let sep = if delivered > 0 { 2 } else { 0 };
        let Some(room) = budget.checked_sub(used + sep + reserve) else {
            break;
        };
        let path = root.join(&n.path).display().to_string();
        let stamp = format!("updated {}", updated(n.mtime));
        let bare = format!("{path} ({stamp})");
        let bare_len = bare.chars().count();
        let entry = if bare_len > room {
            break;
        } else {
            // `{path} ({title} — {stamp})` adds the title plus three characters.
            let title_room = (room - bare_len).saturating_sub(3).min(TITLE_CHARS);
            let title: String = n.title.chars().take(title_room).collect();
            if title.is_empty() {
                bare
            } else {
                format!("{path} ({title} — {stamp})")
            }
        };
        if sep > 0 {
            text.push_str("; ");
        }
        used += sep + entry.chars().count();
        text.push_str(&entry);
        delivered += 1;
    }
    if delivered == 0 {
        return None;
    }
    let rest = notices.len() - delivered;
    let suffix = if rest > 0 { more(rest) } else { String::new() };
    Some(Note {
        text: format!("{HEAD}{text}{suffix}{TAIL}"),
        delivered,
    })
}

/// Turn/session-start check for hooks: returns a note of at most `budget` characters
/// and marks only the pages it names as seen, so the rest arrive on later turns.
/// Without a hook payload (`cwd` and `session` both empty) it does nothing. Errors,
/// such as a busy `usage.sqlite`, are reported on stderr, deliver nothing and leave
/// the seen state unchanged; they never suppress the rest of the hook output.
pub fn hook(
    root: &Path,
    index: &Connection,
    cwd: Option<&str>,
    session: &str,
    budget: usize,
) -> Option<String> {
    let bot_home = std::env::var_os("ORCA_BOT_HOME").map(PathBuf::from);
    hook_with(root, index, bot_home.as_deref(), cwd, session, budget)
}

fn hook_with(
    root: &Path,
    index: &Connection,
    bot_home: Option<&Path>,
    cwd: Option<&str>,
    session: &str,
    budget: usize,
) -> Option<String> {
    let cwd = match cwd.filter(|s| !s.is_empty()) {
        Some(cwd) => Some(PathBuf::from(cwd)),
        None if session.is_empty() => return None,
        None => std::env::current_dir().ok(),
    };
    let key = consumer_key(bot_home, cwd.as_deref(), session)?;
    let result = (|| -> Result<Option<String>> {
        if notices(index, None)?.is_empty() {
            return Ok(None);
        }
        let db = open_seen(root)?;
        let unseen: Vec<_> = notices(index, Some((&db, &key)))?
            .into_iter()
            .filter(Notice::unseen)
            .collect();
        let root = root.canonicalize()?;
        let Some(note) = message(&root, &unseen, budget) else {
            return Ok(None);
        };
        let delivered = &unseen[..note.delivered];
        mark_seen(&db, &key, delivered)?;
        let paths: Vec<_> = delivered.iter().map(|n| n.path.clone()).collect();
        crate::usage::log(&root, session, "bulletin", &paths);
        Ok(Some(note.text))
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
        fs::{self, File},
        os::unix::fs::symlink,
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

    fn check_budget(wiki: &Wiki, cwd: &str, session: &str, budget: usize) -> Option<String> {
        hook_with(&wiki.root, &wiki.index, None, Some(cwd), session, budget)
    }

    fn check(wiki: &Wiki, session: &str) -> Option<String> {
        check_budget(wiki, "/srv/project", session, MAX_CHARS)
    }

    fn seen_paths(wiki: &Wiki, key: &str) -> Result<Vec<String>> {
        Ok(open_seen(&wiki.root)?
            .prepare(
                "select page_path from bulletin_seen where consumer_key = ? order by page_path",
            )?
            .query_map([key], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    fn notice(path: &str, title: &str, mtime: i64) -> Notice {
        Notice {
            path: path.into(),
            title: title.into(),
            mtime,
            seen: None,
        }
    }

    #[test]
    fn consumer_key_prefers_bot_home_then_session() {
        let bot = Path::new("/nonexistent-cw/u/.orca/bots/bot-3/workspace/sub");
        assert_eq!(
            consumer_key(None, Some(bot), "s1").as_deref(),
            Some("bot:bot-3")
        );
        // ORCA_BOT_HOME decides alone, even against a cwd in another bot.
        let bot2 = Path::new("/nonexistent-cw/u/.orca/bots/bot-2");
        assert_eq!(
            consumer_key(Some(bot2), Some(bot), "s1").as_deref(),
            Some("bot:bot-2")
        );
        assert_eq!(
            consumer_key(Some(Path::new("/srv/not-a-bot")), Some(bot), "s1").as_deref(),
            Some("session:s1")
        );
        // Empty ORCA_BOT_HOME counts as unset.
        assert_eq!(
            consumer_key(Some(Path::new("")), Some(bot), "").as_deref(),
            Some("bot:bot-3")
        );
        assert_eq!(
            consumer_key(
                None,
                Some(Path::new("/nonexistent-cw/u/nixos-config")),
                "s1"
            )
            .as_deref(),
            Some("session:s1")
        );
        // A directory merely named like a bot is not a bot home.
        assert_eq!(
            consumer_key(
                None,
                Some(Path::new("/nonexistent-cw/orca/bots/bot-3")),
                "s1"
            )
            .as_deref(),
            Some("session:s1")
        );
        assert_eq!(
            consumer_key(None, Some(Path::new("/x/.orca/bots")), ""),
            None
        );
        assert_eq!(consumer_key(None, Some(Path::new("")), ""), None);
        assert_eq!(consumer_key(None, None, ""), None);
    }

    #[test]
    fn resolution_follows_symlinks_and_parent_components() -> Result<()> {
        let wiki = Wiki::new()?;
        let t = wiki.root.canonicalize()?;
        let bots = t.join(".orca/bots");
        fs::create_dir_all(bots.join("bot-3/work"))?;
        fs::create_dir_all(bots.join("bot-2/sub"))?;
        fs::create_dir_all(t.join("project"))?;
        symlink(t.join("project"), bots.join("bot-3/alias"))?;
        symlink(bots.join("bot-3/work"), t.join("link"))?;
        symlink(bots.join("bot-2/sub"), bots.join("bot-3/cross"))?;
        let key = |p: PathBuf| consumer_key(None, Some(&p), "s");
        let bot3 = Some("bot:bot-3".to_owned());
        let session = Some("session:s".to_owned());
        // Existing paths.
        assert_eq!(key(bots.join("bot-3/work")), bot3);
        assert_eq!(key(bots.join("bot-3/../../../project")), session);
        assert_eq!(key(bots.join("bot-3/alias")), session);
        assert_eq!(key(bots.join("bot-3/alias/../project")), session);
        assert_eq!(key(t.join("link")), bot3);
        assert_eq!(key(bots.join("bot-3/cross")), Some("bot:bot-2".into()));
        // Missing tails: `..` is applied, never dropped.
        assert_eq!(key(bots.join("bot-3/missing/../../../../project")), session);
        assert_eq!(key(bots.join("bot-3/missing/x/../y")), bot3);
        assert_eq!(key(bots.join("bot-3/missing/../work/new")), bot3);
        assert_eq!(key(t.join("link/missing/..")), bot3);
        assert_eq!(key(bots.join("bot-3/alias/missing")), session);
        assert_eq!(key(t.join("nope/.orca/bots/bot-9/../../../x")), session);
        assert_eq!(resolve(Path::new("/../..")), Some(PathBuf::from("/")));
        assert_eq!(
            resolve(&bots.join("bot-3/missing/./a/../b")),
            Some(bots.join("bot-3/missing/b"))
        );
        // ORCA_BOT_HOME uses the same resolution.
        assert_eq!(
            consumer_key(
                Some(&bots.join("bot-3/alias")),
                Some(&bots.join("bot-3")),
                "s"
            ),
            session
        );
        assert_eq!(consumer_key(Some(&t.join("link")), None, "s"), bot3);
        Ok(())
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
        assert!(note.starts_with(HEAD));
        assert!(note.contains("pages/knowledge/notice.md (Team notice — updated "));
        assert!(!note.contains("plain.md") && !note.contains("more"));
        // seen_mtime == page.mtime: silent, also after an index rebuild.
        assert_eq!(check(&wiki, "s1"), None);
        crate::index::remove_index(&wiki.root, "index.sqlite")?;
        wiki.index = crate::index::open(&wiki.root)?;
        crate::index::refresh(&wiki.root, &mut wiki.index)?;
        assert_eq!(check(&wiki, "s1"), None);
        // Another consumer still gets it.
        assert!(check(&wiki, "s2").is_some());
        // An edit re-triggers it, also an mtime that moves backwards.
        touch(&mut wiki, "pages/knowledge/notice.md", 2_000)?;
        assert!(check(&wiki, "s1").is_some());
        assert_eq!(check(&wiki, "s1"), None);
        touch(&mut wiki, "pages/knowledge/notice.md", 1_500)?;
        assert!(check(&wiki, "s1").is_some());
        let db = open_seen(&wiki.root)?;
        let seen: i64 = db.query_row(
            "select seen_mtime from bulletin_seen where consumer_key='session:s1'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(seen, 1_500_000_000_000);
        let logged: i64 = db.query_row(
            "select count(*) from injections where source='bulletin' and session_id='s1'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(logged, 3);
        Ok(())
    }

    #[test]
    fn bot_consumers_share_state_across_sessions() -> Result<()> {
        let mut wiki = Wiki::new()?;
        wiki.page("pages/knowledge/notice.md", BULLETIN)?;
        let bot = "/nonexistent-cw/.orca/bots/bot-3";
        assert!(check_budget(&wiki, bot, "first", MAX_CHARS).is_some());
        assert_eq!(check_budget(&wiki, bot, "second", MAX_CHARS), None);
        assert!(check(&wiki, "second").is_some());
        // No hook payload at all: no state change.
        assert_eq!(
            hook_with(&wiki.root, &wiki.index, None, None, "", MAX_CHARS),
            None
        );
        Ok(())
    }

    #[test]
    fn blocked_deleted_and_untagged_pages_are_never_announced() -> Result<()> {
        let mut wiki = Wiki::new()?;
        wiki.page("pages/knowledge/notice.md", BULLETIN)?;
        wiki.index.execute(
            "update files set blocked='secret' where path='pages/knowledge/notice.md'",
            [],
        )?;
        assert_eq!(check(&wiki, "s1"), None);
        assert!(notices(&wiki.index, None)?.is_empty());
        // A real secret blocks it through the scanner, also after it was seen.
        wiki.page("pages/knowledge/notice.md", BULLETIN)?;
        assert!(check(&wiki, "s2").is_some());
        wiki.page(
            "pages/knowledge/notice.md",
            &format!("{BULLETIN}AKIA{}\n", "ABCDEFGHIJKLMNOP"),
        )?;
        let blocked: String = wiki.index.query_row(
            "select blocked from files where path='pages/knowledge/notice.md'",
            [],
            |r| r.get(0),
        )?;
        assert!(!blocked.is_empty());
        assert_eq!(check(&wiki, "s2"), None);
        assert_eq!(check(&wiki, "s3"), None);
        // Untagged.
        wiki.page(
            "pages/knowledge/notice.md",
            "---\ntitle: Team notice\ncreated: 2026-11-26\ntags: [orca]\n---\nbody v2\n",
        )?;
        assert_eq!(check(&wiki, "s4"), None);
        // Deleted.
        wiki.page("pages/knowledge/other.md", BULLETIN)?;
        fs::remove_file(wiki.root.join("pages/knowledge/other.md"))?;
        crate::index::refresh(&wiki.root, &mut wiki.index)?;
        assert_eq!(check(&wiki, "s5"), None);
        Ok(())
    }

    #[test]
    fn edits_that_keep_the_mtime_are_not_detected() -> Result<()> {
        // Documented limitation: seen state compares mtime only.
        let mut wiki = Wiki::new()?;
        wiki.page("pages/knowledge/notice.md", BULLETIN)?;
        touch(&mut wiki, "pages/knowledge/notice.md", 1_000)?;
        assert!(check(&wiki, "s1").is_some());
        fs::write(
            wiki.root.join("pages/knowledge/notice.md"),
            format!("{BULLETIN}more\n"),
        )?;
        touch(&mut wiki, "pages/knowledge/notice.md", 1_000)?;
        assert_eq!(check(&wiki, "s1"), None);
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
    fn message_stays_within_budget_for_many_long_multibyte_pages() {
        let root = Path::new("/very/long/root").join("디렉터리".repeat(20));
        let pages: Vec<_> = (0..66)
            .map(|i| {
                notice(
                    &format!("pages/knowledge/공지-{i}-{}.md", "😀".repeat(i % 7)),
                    &format!("{i} 한국어 공지 제목 🚀 {}", "긴".repeat(300)),
                    1_000_000_000_000,
                )
            })
            .collect();
        for budget in [0, 50, 200, 300, 400, 600, 1200, MAX_CHARS, 100_000] {
            let Some(note) = message(&root, &pages, budget) else {
                assert!(budget < 400, "budget {budget} fits one page");
                continue;
            };
            assert!(note.text.chars().count() <= budget, "budget {budget}");
            assert!(note.delivered >= 1 && note.delivered <= MAX_PAGES);
            assert!(note.text.contains(&format!(
                " +{} more (run `agent-wiki bulletin`)",
                66 - note.delivered
            )));
            assert!(note.text.starts_with(HEAD) && note.text.ends_with(TAIL));
            for (i, page) in pages.iter().take(note.delivered).enumerate() {
                assert!(
                    note.text.contains(&page.path),
                    "page {i} at budget {budget}"
                );
            }
        }
        assert_eq!(
            message(&root, &pages, 100_000).map(|n| n.delivered),
            Some(MAX_PAGES)
        );
        // Everything fits: no suffix.
        let few = &pages[..2];
        let note = message(&root, few, 100_000).expect("fits");
        assert_eq!(note.delivered, 2);
        assert!(!note.text.contains("more (run"));
        assert!(message(&root, &[], 100_000).is_none());
    }

    #[test]
    fn deferred_pages_arrive_on_later_turns() -> Result<()> {
        let mut wiki = Wiki::new()?;
        for i in 0..25 {
            let rel = format!("pages/knowledge/n{i:02}.md");
            wiki.page(&rel, BULLETIN)?;
            touch(&mut wiki, &rel, 1_000 + i)?;
        }
        let first = check(&wiki, "s1").expect("turn 1");
        assert!(first.contains(" +15 more (run `agent-wiki bulletin`)"));
        let seen = seen_paths(&wiki, "session:s1")?;
        assert_eq!(seen.len(), MAX_PAGES);
        // Newest first: n24..n15.
        assert_eq!(seen[0], "pages/knowledge/n15.md");
        assert!(check(&wiki, "s1").expect("turn 2").contains(" +5 more"));
        let third = check(&wiki, "s1").expect("turn 3");
        assert!(!third.contains("more (run"));
        assert_eq!(seen_paths(&wiki, "session:s1")?.len(), 25);
        assert_eq!(check(&wiki, "s1"), None);
        let logged: i64 = open_seen(&wiki.root)?.query_row(
            "select count(*) from injections where source='bulletin' and session_id='s1'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(logged, 25);
        // A budget too small for a single page delivers nothing and marks nothing.
        assert_eq!(check_budget(&wiki, "/srv/p", "s2", 100), None);
        assert!(seen_paths(&wiki, "session:s2")?.is_empty());
        Ok(())
    }

    #[test]
    fn busy_usage_db_fails_open_without_marking() -> Result<()> {
        let mut wiki = Wiki::new()?;
        wiki.page("pages/knowledge/notice.md", BULLETIN)?;
        open_seen(&wiki.root)?;
        let holder = Connection::open(wiki.root.join("usage.sqlite"))?;
        holder.execute_batch("begin exclusive")?;
        let started = std::time::Instant::now();
        assert_eq!(check(&wiki, "s1"), None);
        assert!(started.elapsed() < Duration::from_secs(2));
        holder.execute_batch("rollback")?;
        assert!(seen_paths(&wiki, "session:s1")?.is_empty());
        assert!(check(&wiki, "s1").is_some());
        assert_eq!(
            seen_paths(&wiki, "session:s1")?,
            ["pages/knowledge/notice.md"]
        );
        Ok(())
    }

    #[test]
    fn status_labels() {
        let mut n = notice("p", "t", 5);
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

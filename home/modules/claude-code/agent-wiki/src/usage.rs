use crate::{expand_home, index, pages, sessions};
use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use clap::Args as ClapArgs;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    time::Duration,
};

#[derive(ClapArgs)]
pub struct Args {
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long)]
    pub json: bool,
}

// v4 table shapes stay compatible; versioned fingerprints rescan old cursors.
const SCAN_FORMAT: i64 = 4;

fn transcript_tables(db: &Connection) -> Result<()> {
    db.execute_batch("create table if not exists tools(tool_use_id text primary key, session_id text, at integer);
        create table if not exists accesses(tool_use_id text, path text, kind text, primary key(tool_use_id, path, kind));
        create index if not exists accesses_path on accesses(path);
        create table if not exists transcripts(path text primary key, offset integer, size integer, mtime integer, fingerprint text);
        create table if not exists sessions(session_id text primary key, first_at integer, last_at integer);
        create table if not exists scripted_sessions(session_id text primary key);
        create table if not exists read_precision(tool_use_id text primary key, precise integer not null);
        create trigger if not exists tools_precision_delete after delete on tools begin delete from read_precision where tool_use_id=old.tool_use_id; end;")?;
    Ok(())
}

pub fn open(root: &Path) -> Result<Connection> {
    let db = sessions::private_db(&root.join("usage.sqlite"))?;
    db.busy_timeout(Duration::from_millis(50))?;
    db.execute_batch("create table if not exists injections(session_id text, path text, source text, at integer);
        create index if not exists injections_session on injections(session_id, path, at);
        create table if not exists injection_clock(id integer primary key, at real);
        create trigger if not exists injections_clock_delete after delete on injections begin delete from injection_clock where id=old.rowid; end; create table if not exists state(key text primary key, value integer);")?;
    transcript_tables(&db)?;
    Ok(db)
}

fn prepare_scan(db: &Connection) -> Result<()> {
    let version = db
        .query_row("select value from state where key='scan_format'", [], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?;
    if version != Some(SCAN_FORMAT) {
        // Only transcript scans rebuild derived data. Hooks never discard injections.
        db.execute_batch(
            "drop table tools; drop table read_precision; drop table accesses; drop table transcripts; drop table sessions; drop table scripted_sessions;
            delete from state where key='scanned';",
        )?;
        transcript_tables(db)?;
        db.execute(
            "insert or replace into state values ('scan_format', ?)",
            [SCAN_FORMAT],
        )?;
    }
    if !db.query_row(
        "select exists(select 1 from state where key='read_precision')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        // Replay available v4 transcripts once to recover explicit .000 precision.
        // Historical tools/injections remain usable even without their sources.
        db.execute_batch(
            "delete from transcripts; insert into state values ('read_precision', 1);",
        )?;
    }
    Ok(())
}

pub fn source(value: &str) -> Result<String, String> {
    if (1..=31).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
    {
        Ok(value.into())
    } else {
        Err("source must match [a-z][a-z-]{0,30}".into())
    }
}

pub fn log(root: &Path, session: &str, source: &str, paths: &[String]) {
    if session.is_empty() || paths.is_empty() {
        return;
    }
    // Measurement must never suppress a hook's output or delay it on a busy DB.
    let _ = (|| -> Result<()> {
        let mut db = open(root)?;
        let tx = db.transaction()?;
        for path in paths {
            let now = Utc::now();
            tx.execute(
                "insert into injections values (?, ?, ?, ?)",
                params![session, path, source, now.timestamp()],
            )?;
            tx.execute(
                "insert into injection_clock values (?,?)",
                params![
                    tx.last_insert_rowid(),
                    now.timestamp_millis() as f64 / 1000.0
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    })();
}

fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

pub(crate) fn page_path(root: &Path, path: &str, cwd: &Path) -> Option<String> {
    let path = expand_home(Path::new(path));
    let absolute = normalized(&if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    });
    crate::compat::roots(root).iter().find_map(|root| {
        let rel = absolute.strip_prefix(normalized(root)).ok()?.to_str()?;
        (!pages::page_type(rel).is_empty() && rel.ends_with(".md")).then(|| rel.to_owned())
    })
}

#[derive(Debug)]
enum ShellToken {
    Word(String),
    Op(&'static str),
}

fn shell_tokens(command: &str) -> Vec<ShellToken> {
    let mut tokens = vec![];
    let mut heredocs: VecDeque<(String, bool)> = VecDeque::new();
    let mut rest = command;
    while let Some(c) = rest.chars().next() {
        if c == '\n' {
            tokens.push(ShellToken::Op(";"));
            rest = &rest[1..];
            // Here-documents start after the command line, in redirection order.
            // Their contents are data even when they contain shell punctuation.
            while let Some((delimiter, strip_tabs)) = heredocs.pop_front() {
                while !rest.is_empty() {
                    let (line, tail) = rest.split_once('\n').unwrap_or((rest, ""));
                    rest = tail;
                    let line = if strip_tabs {
                        line.trim_start_matches('\t')
                    } else {
                        line
                    };
                    if line == delimiter {
                        break;
                    }
                }
            }
        } else if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else if c == '#' {
            rest = rest.find('\n').map_or("", |i| &rest[i..]);
        } else if let Some(op) = [
            "&>>", "<<-", "<<<", ">>", "<<", ">|", ">&", "<&", "<>", "&>", "&&", "||", ";", "|",
            ">", "<", "&", "(", ")",
        ]
        .into_iter()
        .find(|op| rest.starts_with(op))
        {
            tokens.push(ShellToken::Op(op));
            rest = &rest[op.len()..];
        } else {
            let mut word = String::new();
            let mut quote = None;
            while let Some(c) = rest.chars().next() {
                if quote.is_none() && (c.is_whitespace() || ";|><&()".contains(c)) {
                    break;
                }
                rest = &rest[c.len_utf8()..];
                if c == '\\' && quote != Some('\'') {
                    if let Some(next) = rest.chars().next() {
                        rest = &rest[next.len_utf8()..];
                        if next != '\n' {
                            word.push(next);
                        }
                    }
                } else if quote == Some(c) {
                    quote = None;
                } else if quote.is_none() && matches!(c, '\'' | '"') {
                    quote = Some(c);
                } else {
                    word.push(c);
                }
            }
            if let Some(ShellToken::Op(op @ ("<<" | "<<-"))) = tokens.last() {
                heredocs.push_back((word.clone(), *op == "<<-"));
            }
            if !word.is_empty() {
                tokens.push(ShellToken::Word(word));
            }
        }
    }
    tokens
}

fn in_place(args: &[&str]) -> bool {
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if *arg == "--" {
            break;
        }
        if matches!(*arg, "-e" | "-f" | "--expression" | "--file") {
            skip = true;
            continue;
        }
        if *arg == "--in-place" || arg.starts_with("--in-place=") {
            return true;
        }
        if arg.starts_with('-') && !arg.starts_with("--") {
            // -ni and -pi are common; anything after -e/-f is a script, not flags.
            for flag in arg[1..].chars() {
                if flag == 'i' {
                    return true;
                }
                if matches!(flag, 'e' | 'f') {
                    break;
                }
            }
        }
    }
    false
}

/// Literal shell write targets only; never evaluate expansions or script bodies.
pub(crate) fn shell_writes(command: &str) -> Vec<String> {
    let tokens = shell_tokens(command);
    let mut paths = vec![];
    for segment in
        tokens.split(|t| matches!(t, ShellToken::Op(";" | "&&" | "||" | "|" | "&" | "(" | ")")))
    {
        for pair in segment.windows(2) {
            if let [
                ShellToken::Op(">" | ">>" | ">|" | "&>" | "&>>"),
                ShellToken::Word(path),
            ] = pair
            {
                paths.push(path.clone());
            }
        }
        let args: Vec<_> = segment
            .iter()
            .filter_map(|t| match t {
                ShellToken::Word(word) => Some(word.as_str()),
                _ => None,
            })
            .collect();
        let first = args.first().copied().unwrap_or_default();
        if first == "tee" {
            paths.extend(
                args.iter()
                    .skip(1)
                    .filter(|a| !a.starts_with('-'))
                    .map(|s| s.to_string()),
            );
        } else if matches!(first, "sed" | "perl") && in_place(&args[1..]) {
            // The final literal operand is the common in-place editing form.
            if let Some(path) = args.last().filter(|p| !p.starts_with('-')) {
                paths.push((*path).to_string());
            }
        }
    }
    paths.retain(|p| {
        !p.contains(['$', '`', '\n'])
            && !p.starts_with("/dev/")
            && p != "-"
            && !p.bytes().all(|b| b.is_ascii_digit())
    });
    paths.sort();
    paths.dedup();
    paths
}

pub(crate) fn bash_accesses(root: &Path, command: &str, cwd: &Path) -> Vec<(String, &'static str)> {
    let mut cwd = cwd.to_path_buf();
    let mut accesses = vec![];
    let tokens = shell_tokens(command);
    for segment in
        tokens.split(|t| matches!(t, ShellToken::Op(";" | "&&" | "||" | "|" | "&" | "(" | ")")))
    {
        let mut args = vec![];
        let mut redirects = vec![];
        let mut i = 0;
        while i < segment.len() {
            match &segment[i] {
                ShellToken::Op(op) => {
                    if let Some(ShellToken::Word(target)) = segment.get(i + 1) {
                        if matches!(*op, ">" | ">>" | ">|" | "&>" | "&>>" | ">&" | "<>") {
                            redirects.push((target.as_str(), "write"));
                        } else if *op == "<" {
                            redirects.push((target.as_str(), "read"));
                        }
                        i += 1;
                    }
                }
                ShellToken::Word(word) => {
                    // A numeric file descriptor before a redirection is not argv.
                    if !(word.bytes().all(|b| b.is_ascii_digit())
                        && matches!(segment.get(i + 1), Some(ShellToken::Op(_))))
                    {
                        args.push(word.as_str());
                    }
                }
            }
            i += 1;
        }
        let first = args.first().copied().unwrap_or_default();
        let reader = ["cat", "sed", "head", "tail", "less", "nl", "awk", "bat"].contains(&first);
        let editing = matches!(first, "sed" | "perl") && in_place(&args[1..]);
        for (path, kind) in redirects {
            if (kind == "write" || reader)
                && let Some(path) = page_path(root, path, &cwd)
            {
                accesses.push((path, kind));
            }
        }
        if first == "cd" {
            if let Some(dir) = args.get(1) {
                let dir = expand_home(Path::new(dir));
                cwd = normalized(&if dir.is_absolute() {
                    dir
                } else {
                    cwd.join(dir)
                });
            }
        } else if reader || first == "tee" || editing {
            let kind = if first == "tee" || editing {
                "write"
            } else {
                "read"
            };
            let mut skip = false;
            let mut options = true;
            for arg in args.iter().skip(1) {
                if skip {
                    skip = false;
                    continue;
                }
                if options && *arg == "--" {
                    options = false;
                    continue;
                }
                if editing && options && matches!(*arg, "-e" | "-f" | "--expression" | "--file") {
                    skip = true;
                    continue;
                }
                if let Some(path) = page_path(root, arg, &cwd) {
                    accesses.push((path, kind));
                }
            }
        }
    }
    accesses.sort();
    accesses.dedup();
    accesses
}

#[derive(Debug)]
struct Access {
    id: String,
    session: String,
    accesses: Vec<(String, &'static str)>,
    at: f64,
    precise: bool,
}

fn timestamp(value: &Value) -> Option<f64> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|t| t.timestamp_millis() as f64 / 1000.0)
}

fn parse(root: &Path, value: &Value) -> Vec<Access> {
    if value["type"] != "assistant" {
        return vec![];
    }
    let (Some(session), Some(at), Some(parts)) = (
        value["sessionId"].as_str(),
        timestamp(&value["timestamp"]),
        value["message"]["content"].as_array(),
    ) else {
        return vec![];
    };
    parts
        .iter()
        .filter_map(|part| {
            if part["type"] != "tool_use" {
                return None;
            }
            let id = part["id"].as_str()?;
            let input = &part["input"];
            let cwd = Path::new(
                input["cwd"]
                    .as_str()
                    .or_else(|| value["cwd"].as_str())
                    .unwrap_or("/"),
            );
            let accesses = match part["name"].as_str()? {
                "Read" | "Write" | "Edit" => page_path(root, input["file_path"].as_str()?, cwd)
                    .map(|path| {
                        (
                            path,
                            if part["name"] == "Read" {
                                "read"
                            } else {
                                "write"
                            },
                        )
                    })
                    .into_iter()
                    .collect(),
                "Bash" => bash_accesses(root, input["command"].as_str()?, cwd),
                _ => return None,
            };
            (!accesses.is_empty()).then(|| Access {
                id: id.into(),
                session: session.into(),
                accesses,
                precise: value["timestamp"].as_str().is_some_and(|s| s.contains('.')),
                at,
            })
        })
        .collect()
}

fn record(db: &Connection, access: Access) -> Result<()> {
    // Upgrade second-rounded observations when a v4 cursor is replayed. Keep
    // historical accesses even when the original transcript was rewritten.
    db.execute(
        "update tools set at=? where tool_use_id=? and session_id=? and at=cast(? as integer)",
        params![access.at, access.id, access.session, access.at],
    )?;
    db.execute("insert into sessions values (?, ?, ?) on conflict(session_id) do update set first_at=min(first_at,excluded.first_at), last_at=max(last_at,excluded.last_at)", params![access.session, access.at as i64, access.at as i64])?;
    if db.execute(
        "insert or ignore into tools values (?, ?, ?)",
        params![access.id, access.session, access.at],
    )? > 0
    {
        for (path, kind) in access.accesses {
            db.execute(
                "insert into accesses values (?, ?, ?)",
                params![access.id, path, kind],
            )?;
        }
    }
    db.execute("insert into read_precision select tool_use_id, ? from tools where tool_use_id=? and session_id=? and at=? on conflict(tool_use_id) do update set precise=max(precise,excluded.precise)", params![access.precise, access.id, access.session, access.at])?;
    Ok(())
}

fn transcript_paths(dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut paths = vec![];
    for dir in dirs {
        paths.extend(sessions::transcript_paths(dir)?);
        for project in fs::read_dir(dir)? {
            let project = project?.path();
            if !project.is_dir() {
                continue;
            }
            for session in fs::read_dir(project)? {
                let subagents = session?.path().join("subagents");
                if !subagents.is_dir() {
                    continue;
                }
                for entry in fs::read_dir(subagents)? {
                    let path = entry?.path();
                    if path.is_file() && path.extension().is_some_and(|s| s == "jsonl") {
                        paths.push(path.canonicalize()?);
                    }
                }
            }
        }
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

// Check the beginning and the last consumed bytes even when a rewritten file grew.
pub(crate) fn fingerprint(file: &mut File, offset: u64) -> Result<String> {
    let mut hash = 0xcbf29ce484222325u64;
    for start in [0, offset.saturating_sub(4096)] {
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0; (offset - start).min(4096) as usize];
        file.read_exact(&mut bytes)?;
        for byte in bytes {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("fnv4k-v1:{hash:016x}"))
}

#[cfg(test)]
fn scan(root: &Path, db: &mut Connection, dirs: &[PathBuf]) -> Result<()> {
    scan_sources(root, db, dirs, &[], &[])
}

fn scan_sources(
    root: &Path,
    db: &mut Connection,
    dirs: &[PathBuf],
    codex_dirs: &[PathBuf],
    ag_dirs: &[PathBuf],
) -> Result<()> {
    let tx = db.transaction()?;
    prepare_scan(&tx)?;
    let mut paths = transcript_paths(dirs)?;
    let mut ag = BTreeMap::new();
    for dir in codex_dirs {
        paths.extend(sessions::codex_paths(dir)?);
    }
    for dir in ag_dirs {
        for (path, meta) in sessions::antigravity_paths(dir)? {
            paths.push(path.clone());
            ag.insert(path, meta);
        }
    }
    paths.sort();
    paths.dedup();
    for path in paths {
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let (mtime, size) = index::stamp(&file.metadata()?);
        let name = path.to_string_lossy();
        let old: Option<(i64, i64, i64, String)> = tx
            .query_row(
                "select offset, size, mtime, fingerprint from transcripts where path = ?",
                [&*name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let mut offset = 0;
        if let Some((previous, old_size, old_mtime, old_hash)) = old {
            if size == old_size && mtime == old_mtime && old_hash.starts_with("fnv4k-v1:") {
                continue;
            }
            if size > old_size && fingerprint(&mut file, previous as u64)? == old_hash {
                offset = previous as u64;
            }
        }
        let codex = crate::transcript::Codex::open(&path)?;
        let mut position = offset;
        let offset = sessions::complete_lines(file.try_clone()?, offset, |line| {
            if let Ok(value) = serde_json::from_slice::<Value>(line) {
                if value["entrypoint"] == "sdk-cli"
                    && let Some(session) = value["sessionId"].as_str()
                {
                    tx.execute(
                        "insert or ignore into scripted_sessions values (?)",
                        [session],
                    )?;
                }
                if let (Some(session), Some(at)) =
                    (value["sessionId"].as_str(), timestamp(&value["timestamp"]))
                {
                    tx.execute("insert into sessions values (?, ?, ?) on conflict(session_id) do update set first_at = min(first_at, excluded.first_at), last_at = max(last_at, excluded.last_at)", params![session, at as i64, at as i64])?;
                }
                let mut accesses = parse(root, &value);
                if let Some(meta) = &codex {
                    accesses.extend(parse_other(
                        root,
                        &value,
                        &meta.session,
                        &meta.cwd,
                        position,
                        false,
                    ));
                } else if let Some(meta) = ag.get(&path) {
                    accesses.extend(parse_other(
                        root,
                        &value,
                        &meta.conversation,
                        &meta.cwd,
                        position,
                        true,
                    ));
                }
                for access in accesses {
                    record(&tx, access)?;
                }
            }
            position += line.len() as u64;
            Ok(())
        })?;
        let hash = fingerprint(&mut file, offset)?;
        tx.execute("insert into transcripts values (?, ?, ?, ?, ?) on conflict(path) do update set offset=excluded.offset, size=excluded.size, mtime=excluded.mtime, fingerprint=excluded.fingerprint", params![name, offset as i64, size, mtime, hash])?;
    }
    tx.execute(
        "insert or replace into state values ('scanned', ?)",
        [Utc::now().timestamp()],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn read_pages(db: &Connection, since: i64) -> Result<HashSet<String>> {
    Ok(db.prepare("select distinct path from accesses join tools using(tool_use_id) where kind = 'read' and at >= ? and session_id not in (select session_id from scripted_sessions)")?
        .query_map([since], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

pub fn scanned(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "select exists(select 1 from state where key = 'scanned') and exists(select 1 from state where key = 'scan_format' and value = ?)",
        [SCAN_FORMAT],
        |r| r.get(0),
    )?)
}

// Serialized fields stay in sorted key order to match the former JSON maps.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PageUsage {
    pub importance: f64,
    pub injected_unread: i64,
    pub last_read: Option<i64>,
    pub read_sessions: i64,
    pub reads: i64,
}

// Explicit transcript fractions retain precision even at .000. Legacy rows
// without provenance infer precision only from a non-integer read time. An
// injection is precise only when its auxiliary clock exists. Coarse comparisons
// use second buckets so a whole-second read cannot be placed before .200.
const READ_AFTER_INJECTION: &str = "(t.at > i.first_at or
    ((not i.first_precise or not coalesce((select precise from read_precision p where p.tool_use_id=t.tool_use_id), t.at!=cast(t.at as integer))) and
     cast(t.at as integer)>=cast(i.first_at as integer)))";

// Select the first injection and its precision together. Rank after filtering
// so source/window-specific stats and all-source page metrics share one rule;
// ties prefer a coarse observation rather than inventing missing precision.
fn first_injections(filter: &str) -> String {
    format!("select session_id,path,first_at,first_precise from (
        select j.session_id,j.path,coalesce(c.at,j.at) as first_at,
        c.at is not null as first_precise,
        row_number() over(partition by j.session_id,j.path order by coalesce(c.at,j.at),c.at is not null,j.rowid) as position
        from injections j left join injection_clock c on c.id=j.rowid
        where {filter} and j.session_id not in (select session_id from scripted_sessions)
        ) where position=1")
}

pub fn per_page(db: &Connection, now: i64) -> Result<BTreeMap<String, PageUsage>> {
    let mut pages = BTreeMap::<String, PageUsage>::new();
    let mut stmt = db.prepare(
        "select path, sum(kind='read'), count(distinct case when kind='read' then session_id end),
         max(case when kind='read' then at end) from accesses join tools using(tool_use_id)
         where session_id not in (select session_id from scripted_sessions) group by path",
    )?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            PageUsage {
                reads: r.get(1)?,
                read_sessions: r.get(2)?,
                last_read: r.get::<_, Option<f64>>(3)?.map(|v| v as i64),
                ..PageUsage::default()
            },
        ))
    })? {
        let (path, usage) = row?;
        pages.insert(path, usage);
    }
    // Every consumer uses the same precision-aware later-read predicate.
    let first = first_injections("1");
    let mut stmt = db.prepare(&format!(
        "select i.path, sum(not exists(select 1 from accesses a join tools t using(tool_use_id)
         where a.path=i.path and t.session_id=i.session_id and a.kind='read' and {READ_AFTER_INJECTION}))
         from ({first}) i group by i.path"
    ))?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (path, count) = row?;
        pages.entry(path).or_default().injected_unread = count;
    }
    for usage in pages.values_mut() {
        if let Some(last_read) = usage.last_read {
            let days = now.saturating_sub(last_read).max(0) as f64 / 86400.0;
            usage.importance = (usage.reads as f64).ln_1p() * (-days / 30.0).exp()
                + 0.5 * (usage.read_sessions as f64).ln_1p()
                - 0.1 * (usage.injected_unread as f64).ln_1p();
        }
    }
    Ok(pages)
}

pub fn refresh(root: &Path) -> Result<Connection> {
    let mut db = open(root)?;
    scan_sources(
        root,
        &mut db,
        &sessions::source_dirs()?,
        &sessions::codex_source_dirs()?,
        &sessions::antigravity_source_dirs()?,
    )?;
    Ok(db)
}

pub fn age_seconds(created: &str, mtime: i64, now: i64) -> i64 {
    let at = NaiveDate::parse_from_str(created, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| d.and_utc().timestamp())
        .unwrap_or(mtime / 1_000_000_000);
    now - at
}

pub fn older_than(created: &str, mtime: i64, now: i64, days: i64) -> bool {
    age_seconds(created, mtime, now) > days * 86400
}

#[derive(Serialize)]
struct Report {
    coverage: Coverage,
    injected_without_read: Vec<UnreadInjection>,
    since: Option<i64>,
    sources: BTreeMap<String, SourceStats>,
    types: BTreeMap<&'static str, TypeStats>,
}

#[derive(Serialize)]
struct Coverage {
    from: Option<i64>,
    scripted_sessions: i64,
    sessions: i64,
    to: Option<i64>,
    transcripts: i64,
}

#[derive(Serialize)]
struct UnreadInjection {
    count: i64,
    path: String,
    title: String,
}

#[derive(Debug, PartialEq, Serialize)]
struct SourceStats {
    injections: usize,
    rate: f64,
    read_later: usize,
}

#[derive(Serialize)]
struct TypeStats {
    cross_session_read: usize,
    ever_read: usize,
    older_than_14d_unread: usize,
    pages: usize,
}

fn aggregate(db: &Connection, index: &Connection, since: i64, now: i64) -> Result<Report> {
    let mut sources = BTreeMap::new();
    let mut ignored = BTreeMap::<String, i64>::new();
    let mut source_names = db
        .prepare("select distinct source from injections where at>=? and session_id not in (select session_id from scripted_sessions)")?
        .query_map([since], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    source_names.extend(["remind", "recall-skill", "recall-failure", "context"].map(String::from));
    for source in source_names {
        let first = first_injections("j.source=? and j.at>=?");
        let rows = db.prepare(&format!("select i.session_id, i.path, exists(select 1 from accesses a join tools t using(tool_use_id) where a.path=i.path and t.session_id=i.session_id and a.kind='read' and {READ_AFTER_INJECTION}) from ({first}) i"))?
            .query_map(params![source, since], |r| Ok((r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let read = rows.iter().filter(|(_, read)| *read).count();
        sources.insert(
            source,
            SourceStats {
                injections: rows.len(),
                rate: if rows.is_empty() {
                    0.0
                } else {
                    read as f64 / rows.len() as f64
                },
                read_later: read,
            },
        );
    }
    // Count distinct session/page suggestions across all sources for the top list.
    let first = first_injections("j.at>=?");
    let rows = db.prepare(&format!("select path, count(*) from ({first}) i where not exists(select 1 from accesses a join tools t using(tool_use_id) where a.path=i.path and t.session_id=i.session_id and a.kind='read' and {READ_AFTER_INJECTION}) group by path"))?
        .query_map([since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    ignored.extend(rows);
    let read = read_pages(db, since)?;
    let ever = read_pages(db, i64::MIN)?;
    let cross: HashSet<String> = db.prepare("select distinct a.path from accesses a join tools t using(tool_use_id) where a.kind='read' and t.at>=? and t.session_id not in (select session_id from scripted_sessions) and not exists(select 1 from accesses w join tools wt using(tool_use_id) where w.path=a.path and wt.session_id=t.session_id and w.kind='write')")?
        .query_map([since], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut types = BTreeMap::new();
    let mut titles = BTreeMap::new();
    for kind in pages::TYPES {
        let rows = index.prepare("select fts.path, fts.title, fts.created, files.mtime_ns, files.blocked from fts join files on files.id=fts.rowid where fts.type=? order by fts.path")?
            .query_map([kind], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, String>(4)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        types.insert(
            kind,
            TypeStats {
                cross_session_read: rows.iter().filter(|r| cross.contains(&r.0)).count(),
                ever_read: rows.iter().filter(|r| read.contains(&r.0)).count(),
                older_than_14d_unread: rows
                    .iter()
                    .filter(|r| !ever.contains(&r.0) && older_than(&r.2, r.3, now, 14))
                    .count(),
                pages: rows.len(),
            },
        );
        for (path, title, _, _, blocked) in rows {
            titles.insert(
                path,
                if blocked.is_empty() {
                    title
                } else {
                    "[BLOCKED]".into()
                },
            );
        }
    }
    let mut top: Vec<_> = ignored.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let top = top
        .into_iter()
        .take(10)
        .map(|(path, count)| UnreadInjection {
            count,
            title: titles.get(&path).unwrap_or(&path).clone(),
            path,
        })
        .collect();
    let (sessions, first, last): (i64, Option<i64>, Option<i64>) = db.query_row(
        "select count(*), min(max(first_at, ?)), max(last_at) from sessions where last_at >= ? and session_id not in (select session_id from scripted_sessions)",
        params![since, since],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let transcripts: i64 = db.query_row("select count(*) from transcripts", [], |r| r.get(0))?;
    let scripted_sessions =
        db.query_row("select count(*) from scripted_sessions", [], |r| r.get(0))?;
    Ok(Report {
        coverage: Coverage {
            from: first,
            scripted_sessions,
            sessions,
            to: last,
            transcripts,
        },
        injected_without_read: top,
        since: (since != i64::MIN).then_some(since),
        sources,
        types,
    })
}

pub fn run(root: &Path, index: &Connection, args: &Args) -> Result<()> {
    let now = Utc::now();
    let since = args
        .since
        .as_ref()
        .map(|s| sessions::date(s, now))
        .transpose()?
        .map(|s| DateTime::parse_from_rfc3339(&s).map(|d| d.timestamp()))
        .transpose()?
        .unwrap_or(i64::MIN);
    let db = refresh(root)?;
    let report = aggregate(&db, index, since, now.timestamp())?;
    if args.json {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    for (source, row) in &report.sources {
        println!(
            "{source}: {} injections, {} read later ({:.1}%)",
            row.injections,
            row.read_later,
            row.rate * 100.0
        );
    }
    for (kind, row) in &report.types {
        println!(
            "{kind}: {} pages, {} read, {} cross-session, {} older than 14d unread",
            row.pages, row.ever_read, row.cross_session_read, row.older_than_14d_unread
        );
    }
    for row in &report.injected_without_read {
        println!(
            "unread injections: {}: {} ({})",
            row.count, row.title, row.path
        );
    }
    let span = &report.coverage;
    let time = |v: Option<i64>| {
        v.and_then(|n| DateTime::from_timestamp(n, 0))
            .map_or_else(|| "none".into(), |d| d.to_rfc3339())
    };
    println!(
        "scanned: {} transcripts, {} sessions; {} to {}; {} scripted sessions excluded",
        span.transcripts,
        span.sessions,
        time(span.from),
        time(span.to),
        span.scripted_sessions
    );
    Ok(())
}

fn parse_other(
    root: &Path,
    value: &Value,
    session: &str,
    cwd: &str,
    position: u64,
    ag: bool,
) -> Vec<Access> {
    let Some(at) = timestamp(&value[if ag { "created_at" } else { "timestamp" }]) else {
        return vec![];
    };
    let payload = if ag { value } else { &value["payload"] };
    let calls = if ag {
        if let Some(calls) = value["tool_calls"].as_array() {
            calls.clone()
        } else if value["status"] == "DONE" {
            vec![value.clone()]
        } else {
            vec![]
        }
    } else if payload["type"] == "function_call" {
        vec![payload.clone()]
    } else {
        vec![]
    };
    calls
        .iter()
        .enumerate()
        .filter_map(|(i, call)| {
            let name = call["name"].as_str().or_else(|| call["type"].as_str())?;
            let input = call
                .get("args")
                .or_else(|| call.get("arguments"))
                .or_else(|| call.get("viewFile"))
                .or_else(|| call.get("writeFile"))
                .or_else(|| call.get("codeAction"))
                .or_else(|| call.get("runCommand"))
                .unwrap_or(call);
            let input = if let Some(s) = input.as_str() {
                serde_json::from_str(s).ok()?
            } else {
                input.clone()
            };
            let cwd = Path::new(
                input["cwd"]
                    .as_str()
                    .or_else(|| input["workdir"].as_str())
                    .unwrap_or(cwd),
            );
            let accesses = match name {
                "exec_command"
                | "shell_command"
                | "shell"
                | "run_command"
                | "CORTEX_STEP_TYPE_RUN_COMMAND" => {
                    let value = ["cmd", "command", "CommandLine", "commandLine"]
                        .iter()
                        .find_map(|k| input.get(*k))?;
                    let command = if let Some(text) = value.as_str() {
                        text.to_owned()
                    } else {
                        let args = value
                            .as_array()?
                            .iter()
                            .map(Value::as_str)
                            .collect::<Option<Vec<_>>>()?;
                        if args.len() == 3
                            && matches!(args[1], "-c" | "-lc")
                            && matches!(
                                Path::new(args[0]).file_name()?.to_str()?,
                                "bash" | "sh" | "zsh"
                            )
                        {
                            args[2].to_owned()
                        } else {
                            args.iter()
                                .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
                                .collect::<Vec<_>>()
                                .join(" ")
                        }
                    };
                    bash_accesses(root, &command, cwd)
                }
                "read_file"
                | "write_file"
                | "edit_file"
                | "CORTEX_STEP_TYPE_VIEW_FILE"
                | "CORTEX_STEP_TYPE_WRITE_FILE"
                | "CORTEX_STEP_TYPE_CODE_ACTION" => {
                    let path = [
                        "Path",
                        "TargetFile",
                        "AbsolutePath",
                        "file_path",
                        "path",
                        "absolutePath",
                        "targetFile",
                    ]
                    .iter()
                    .find_map(|k| input[*k].as_str())?;
                    page_path(root, path, cwd)
                        .map(|path| {
                            (
                                path,
                                if matches!(name, "read_file" | "CORTEX_STEP_TYPE_VIEW_FILE") {
                                    "read"
                                } else {
                                    "write"
                                },
                            )
                        })
                        .into_iter()
                        .collect()
                }
                _ => return None,
            };
            if accesses.is_empty() {
                return None;
            }
            Some(Access {
                id: format!(
                    "{}:{session}:{}:{i}",
                    if ag { "ag" } else { "codex" },
                    call["call_id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| position.to_string())
                ),
                session: session.into(),
                accesses,
                precise: value[if ag { "created_at" } else { "timestamp" }]
                    .as_str()
                    .is_some_and(|s| s.contains('.')),
                at,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    #[test]
    fn explicit_zero_fraction_retains_precise_ordering() -> anyhow::Result<()> {
        let mut wiki = Wiki::new()?;
        let path = "pages/knowledge/zero.md";
        wiki.page(path, "---\ntitle: 영 시각\n---\n검증\n")?;
        let db = open(&wiki.root)?;
        db.execute(
            "insert into injections values ('s',?,'context',1795737600)",
            [path],
        )?;
        db.execute_batch("insert into injection_clock values (1,1795737600.2);")?;
        let value = serde_json::json!({"type":"assistant","sessionId":"s","timestamp":"2026-11-27T00:00:00.000Z","message":{"content":[{"type":"tool_use","id":"r","name":"Read","input":{"file_path":wiki.root.join(path)}}]}});
        for access in parse(&wiki.root, &value) {
            record(&db, access)?;
        }
        assert_eq!(
            aggregate(&db, &wiki.index, 0, 1795737601)?.sources["context"].read_later,
            0
        );
        assert_eq!(per_page(&db, 1795737601)?[path].injected_unread, 1);
        Ok(())
    }

    #[test]
    fn null_injection_clock_uses_coarse_fallback_with_a_precise_zero_fraction_read()
    -> anyhow::Result<()> {
        let mut wiki = Wiki::new()?;
        let path = "pages/knowledge/clock.md";
        wiki.page(
            path,
            "---\ntitle: 시각 검증\ncreated: 2026-11-27\n---\n검증\n",
        )?;
        let db = open(&wiki.root)?;
        db.execute(
            "insert into injections values ('s',?,'context',1795737600)",
            [path],
        )?;
        db.execute_batch(
            "insert into injection_clock values (last_insert_rowid(), NULL);
            insert into tools values ('r','s',1795737600);
            insert into read_precision values ('r',1);",
        )?;
        db.execute("insert into accesses values ('r',?,'read')", [path])?;
        let report = aggregate(&db, &wiki.index, 0, 1795737601)?;
        assert_eq!(report.sources["context"].read_later, 1);
        assert!(report.injected_without_read.is_empty());
        assert_eq!(per_page(&db, 1795737601)?[path].injected_unread, 0);
        Ok(())
    }

    #[test]
    fn precision_fallback_is_consistent_for_all_consumers() -> anyhow::Result<()> {
        for (clock, read, expected) in [
            (None, 1795737600.0, true),
            (Some(1795737600.2), 1795737600.0, true),
            (Some(1795737600.2), 1795737600.1, false),
            (Some(1795737600.2), 1795737600.3, true),
            (None, 1795737599.0, false),
            (Some(1795737600.2), 1795737599.9, false),
            (Some(1795737600.2), 1795737600.2, false),
        ] {
            let mut wiki = Wiki::new()?;
            let path = "pages/knowledge/precision.md";
            wiki.page(
                path,
                "---\ntitle: 시각 검증\ncreated: 2026-11-27\n---\n검증\n",
            )?;
            let db = open(&wiki.root)?;
            db.execute(
                "insert into injections values ('s',?,'context',1795737600)",
                [path],
            )?;
            if let Some(clock) = clock {
                db.execute("insert into injection_clock values (1,?)", [clock])?;
            }
            db.execute("insert into tools values ('t','s',?)", [read])?;
            db.execute("insert into accesses values ('t',?,'read')", [path])?;
            let report = aggregate(&db, &wiki.index, 0, 1795737601)?;
            assert_eq!(
                report.sources["context"].read_later,
                usize::from(expected),
                "{clock:?} {read}"
            );
            assert_eq!(report.injected_without_read.is_empty(), expected);
            assert_eq!(
                per_page(&db, 1795737601)?[path].injected_unread,
                i64::from(!expected)
            );
        }
        Ok(())
    }

    #[test]
    fn cross_harness_accesses_keep_subsecond_order() {
        use serde_json::json;
        let root = std::path::Path::new("/wiki");
        let codex = json!({"timestamp":"2026-11-27T00:00:00.100Z", "payload":{"type":"function_call", "name":"exec_command", "call_id":"a", "arguments":"{\"cmd\":\"cat /wiki/pages/knowledge/a.md\"}"}});
        let result = super::parse_other(root, &codex, "session", "/", 0, false);
        assert_eq!(
            result[0].accesses,
            vec![("pages/knowledge/a.md".into(), "read")]
        );
        assert!(result[0].at > 1795737600.0);
        let shell = json!({"timestamp":"2026-11-27T00:00:00.100Z", "payload":{"type":"function_call", "name":"shell", "call_id":"b", "arguments":{"command":["bash","-lc","cat /wiki/pages/knowledge/a.md"]}}});
        assert_eq!(
            super::parse_other(root, &shell, "session", "/", 0, false)[0].accesses[0].1,
            "read"
        );
        let cortex = json!({"created_at":"2026-11-27T00:00:00.100Z", "status":"DONE", "type":"CORTEX_STEP_TYPE_VIEW_FILE", "viewFile":{"absolutePath":"/wiki/pages/knowledge/a.md"}});
        assert_eq!(
            super::parse_other(root, &cortex, "ag", "/", 0, true)[0].accesses[0].1,
            "read"
        );
        for name in ["read_file", "CORTEX_STEP_TYPE_VIEW_FILE"] {
            let ag = json!({"created_at":"2026-11-27T00:00:00.050Z", "status":"DONE", "tool_calls":[{"name":name,"args":{"Path":"/wiki/pages/knowledge/a.md"}}]});
            let result = super::parse_other(root, &ag, "ag", "/", 0, true);
            assert_eq!(result[0].accesses[0].1, "read");
            assert!(result[0].at < 1795737600.1);
        }
    }

    use super::*;
    use serde_json::json;
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };

    pub struct Wiki {
        pub root: PathBuf,
        pub index: Connection,
    }
    impl Wiki {
        pub fn new() -> Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "cw-usage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            crate::ensure_layout(&root)?;
            let index = index::open(&root)?;
            Ok(Self { root, index })
        }
        pub fn page(&mut self, rel: &str, text: &str) -> Result<()> {
            fs::write(self.root.join(rel), text)?;
            index::refresh(&self.root, &mut self.index)
        }
    }
    impl Drop for Wiki {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn line(id: &str, name: &str, input: Value) -> Value {
        json!({"type": "assistant", "sessionId": "parent", "timestamp": "2026-09-01T01:00:00Z", "message": {"content": [{"type":"tool_use", "id":id, "name":name, "input":input}]}})
    }

    #[test]
    fn report_json_preserves_key_order_and_nulls() -> Result<()> {
        let mut report = Report {
            coverage: Coverage {
                from: Some(10),
                scripted_sessions: 1,
                sessions: 2,
                to: Some(20),
                transcripts: 3,
            },
            injected_without_read: vec![UnreadInjection {
                count: 2,
                path: "a".into(),
                title: "A".into(),
            }],
            since: Some(10),
            sources: BTreeMap::from([
                (
                    "remind".into(),
                    SourceStats {
                        injections: 2,
                        rate: 0.5,
                        read_later: 1,
                    },
                ),
                (
                    "context".into(),
                    SourceStats {
                        injections: 0,
                        rate: 0.0,
                        read_later: 0,
                    },
                ),
            ]),
            types: BTreeMap::from([(
                "knowledge",
                TypeStats {
                    cross_session_read: 1,
                    ever_read: 2,
                    older_than_14d_unread: 3,
                    pages: 5,
                },
            )]),
        };
        assert_eq!(
            serde_json::to_string(&report)?,
            concat!(
                r#"{"coverage":{"from":10,"scripted_sessions":1,"sessions":2,"to":20,"transcripts":3},"#,
                r#""injected_without_read":[{"count":2,"path":"a","title":"A"}],"since":10,"#,
                r#""sources":{"context":{"injections":0,"rate":0.0,"read_later":0},"remind":{"injections":2,"rate":0.5,"read_later":1}},"#,
                r#""types":{"knowledge":{"cross_session_read":1,"ever_read":2,"older_than_14d_unread":3,"pages":5}}}"#,
            )
        );
        report.coverage = Coverage {
            from: None,
            scripted_sessions: 0,
            sessions: 0,
            to: None,
            transcripts: 0,
        };
        report.injected_without_read.clear();
        report.since = None;
        report.sources.clear();
        report.types.clear();
        assert_eq!(
            serde_json::to_string(&report)?,
            r#"{"coverage":{"from":null,"scripted_sessions":0,"sessions":0,"to":null,"transcripts":0},"injected_without_read":[],"since":null,"sources":{},"types":{}}"#
        );
        Ok(())
    }

    #[test]
    fn page_usage_counts_sessions_later_reads_and_decay() -> Result<()> {
        let wiki = Wiki::new()?;
        let db = open(&wiki.root)?;
        assert!(per_page(&db, 100)?.is_empty());
        db.execute_batch(
            "insert into tools values ('r1','s1',50), ('r2','s1',100), ('r3','s2',100),
             ('w1','s3',200), ('w2','s4',200);
             insert into accesses values ('r1','used','read'), ('r2','used','read'),
             ('r3','used','read'), ('w1','used','write'), ('w2','write-only','write');
             insert into injections values
             ('s1','used','remind',75), ('s1','used','context',150),
             ('s2','used','remind',100), ('s2','used','context',100),
             ('s3','used','remind',100), ('s4','used','remind',101),
             ('s5','unread','remind',1), ('s5','unread','context',2);",
        )?;
        let report = per_page(&db, 100)?;
        let used = &report["used"];
        assert_eq!(used.reads, 3);
        assert_eq!(used.read_sessions, 2);
        assert_eq!(used.last_read, Some(100));
        // Whole-second ties count; earlier reads and writes do not.
        assert_eq!(used.injected_unread, 2);
        let baseline = 0.5 * 3_f64.ln() - 0.1 * 3_f64.ln();
        assert!((used.importance - (4_f64.ln() + baseline)).abs() < 1e-12);
        for (days, decay) in [(15, (-0.5_f64).exp()), (30, (-1_f64).exp())] {
            let later = per_page(&db, 100 + days * 86400)?;
            assert!((later["used"].importance - (4_f64.ln() * decay + baseline)).abs() < 1e-12);
        }
        // A clock skew must not amplify reads in the future.
        assert_eq!(per_page(&db, 0)?["used"].importance, used.importance);
        for path in ["unread", "write-only"] {
            assert_eq!(report[path].reads, 0);
            assert_eq!(report[path].read_sessions, 0);
            assert_eq!(report[path].last_read, None);
            assert_eq!(report[path].importance, 0.0);
        }
        assert_eq!(report["unread"].injected_unread, 1);
        assert_eq!(report["write-only"].injected_unread, 0);
        Ok(())
    }

    #[test]
    fn read_write_and_reader_segments_recognize_only_wiki_paths() {
        let root = crate::home().join(".local/share/agent-wiki");
        let rel = "pages/knowledge/test.md";
        for name in ["Read", "Write", "Edit"] {
            for path in [
                root.join(rel).to_string_lossy().into_owned(),
                format!("~/.local/share/agent-wiki/{rel}"),
            ] {
                let access = parse(&root, &line("id", name, json!({"file_path":path})));
                assert_eq!(
                    access[0].accesses,
                    [(rel.into(), if name == "Read" { "read" } else { "write" })]
                );
            }
        }
        for reader in ["cat", "sed", "head", "tail", "less", "nl", "awk", "bat"] {
            let command = format!(
                "echo ignored; cd '{}' && {reader} -n '{rel}' | cat /elsewhere/{rel}",
                root.display()
            );
            assert_eq!(
                parse(&root, &line("id", "Bash", json!({"command":command})))[0].accesses,
                [(rel.into(), "read")]
            );
        }
        for command in [
            format!("echo {}/{}", root.display(), rel),
            format!("cat {rel}"),
            format!("echo x | rg {}/{}", root.display(), rel),
        ] {
            assert!(parse(&root, &line("id", "Bash", json!({"command":command}))).is_empty());
        }
        let mut value = line("id", "Read", json!({"file_path":rel}));
        value["cwd"] = json!(root);
        assert_eq!(parse(&root, &value)[0].accesses, [(rel.into(), "read")]);
        assert!(page_path(&root, "pages/knowledge/../../../outside.md", &root).is_none());
        assert_eq!(
            bash_accesses(&root, &format!("cat {rel} pages/followups/next.md"), &root).len(),
            2
        );
    }

    #[test]
    fn shell_redirects_tee_and_in_place_edits_record_writes() {
        let root = Path::new("/wiki");
        let page = "pages/knowledge/page.md";
        let source = "pages/knowledge/source.md";
        for command in [
            format!("cat > {page}"),
            format!("cat >>{page}"),
            format!("cat>{page}"),
            format!("printf '%s' text >'{page}'"),
            format!("cat 2>>{page}"),
            format!("tee -a {page}"),
            format!("tee -- {page}"),
            format!("sed -i 's/old/new/' {page}"),
            format!("sed -i.bak -e 's/old/new/' {page}"),
            format!("sed -ni 's/old/new/p' {page}"),
            format!("sed --in-place=.bak --expression 's/old/new/' {page}"),
            format!("perl -pi -e 's/old/new/' {page}"),
            format!("perl -i.bak -pe 's/old/new/' {page}"),
        ] {
            assert_eq!(
                bash_accesses(root, &command, root),
                [(page.into(), "write")],
                "{command}"
            );
        }
        for command in [
            format!("cat {source} >{page}"),
            format!("cat <'{source}' >> {page}"),
            format!("cat {source} | tee -a {page}"),
            format!("cat {source}; perl -pi -e 's/old/new/' {page}"),
            format!("2>{page} cat {source}"),
        ] {
            assert_eq!(
                bash_accesses(root, &command, root),
                [(page.into(), "write"), (source.into(), "read")],
                "{command}"
            );
        }
        for command in [
            format!("sed -e 's/i/o/;s/o/i/' {page}"),
            format!("sed -e's/i/o/' {page}"),
            format!("cat '{page}' # >{page}"),
        ] {
            assert_eq!(
                bash_accesses(root, &command, root),
                [(page.into(), "read")],
                "{command}"
            );
        }
        let default_root = crate::home().join(".local/share/agent-wiki");
        assert_eq!(
            bash_accesses(
                &default_root,
                &format!("cat >>~/.local/share/agent-wiki/{page}"),
                Path::new("/")
            ),
            [(page.into(), "write")]
        );
        assert!(bash_accesses(root, &format!("echo 'cat {page}; tee {page}'"), root).is_empty());
    }

    #[test]
    fn heredoc_bodies_are_data_and_commands_resume_after_the_terminator() {
        let root = Path::new("/wiki");
        let page = "pages/knowledge/page.md";
        let source = "pages/knowledge/source.md";
        for marker in ["<<'EOF'", "<<EOF", "<<-EOF", "<<\"EOF\"", "<<\\EOF"] {
            let command =
                format!("cat >{page} {marker}\ncat {source}; tee {source}\nEOF\ncat {source}");
            assert_eq!(
                bash_accesses(root, &command, root),
                [(page.into(), "write"), (source.into(), "read")],
                "{marker}"
            );
        }
        let command =
            format!("python3 <<'PY'\nprint('text'); cat {page}\ncat {source} | tee {page}\nPY");
        assert!(bash_accesses(root, &command, root).is_empty());
        let command =
            format!("cat <<A <<-'B' >{page}\ncat {source}\nA\ntee {source}\n\tB\ncat {source}");
        assert_eq!(
            bash_accesses(root, &command, root),
            [(page.into(), "write"), (source.into(), "read")]
        );
        let command = format!("cat >{page} <<EOF\ncat {source}\n");
        assert_eq!(
            bash_accesses(root, &command, root),
            [(page.into(), "write")]
        );
        assert_eq!(
            bash_accesses(root, &format!("echo '<<EOF'; cat {source}"), root),
            [(source.into(), "read")]
        );
    }

    #[test]
    fn mixed_accesses_deduplicate_per_tool_page_and_kind_and_exclude_writers() -> Result<()> {
        let mut wiki = Wiki::new()?;
        for slug in ["source", "target"] {
            wiki.page(
                &format!("pages/knowledge/{slug}.md"),
                "---\ntitle: Test\ncreated: 2020-01-01\n---\nbody",
            )?;
        }
        let db = open(&wiki.root)?;
        db.execute(
            "insert into injections values ('parent', 'pages/knowledge/target.md', 'remind', 1)",
            [],
        )?;
        let command = format!(
            "cat {}/pages/knowledge/source.md | tee {}/pages/knowledge/target.md",
            wiki.root.display(),
            wiki.root.display()
        );
        let value = line("mixed", "Bash", json!({"command": command}));
        for _ in 0..2 {
            for access in parse(&wiki.root, &value) {
                record(&db, access)?;
            }
        }
        assert_eq!(
            db.query_row("select count(*) from tools", [], |r| r.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            db.query_row("select count(*) from accesses", [], |r| r.get::<_, i64>(0))?,
            2
        );
        let report = aggregate(&db, &wiki.index, i64::MIN, 1800000000)?;
        assert_eq!(report.types["knowledge"].ever_read, 1);
        assert_eq!(report.types["knowledge"].cross_session_read, 1);
        assert_eq!(report.sources["remind"].read_later, 0);
        let command = format!(
            "cd '{}' && cat pages/knowledge/target.md; sed -i 's/a/b/' pages/knowledge/target.md",
            wiki.root.display()
        );
        for access in parse(
            &wiki.root,
            &line("both-kinds", "Bash", json!({"command": command})),
        ) {
            record(&db, access)?;
        }
        assert_eq!(
            db.query_row(
                "select count(*) from accesses where tool_use_id='both-kinds'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            2
        );
        let report = aggregate(&db, &wiki.index, i64::MIN, 1800000000)?;
        assert_eq!(report.types["knowledge"].ever_read, 2);
        assert_eq!(report.types["knowledge"].cross_session_read, 1);
        assert_eq!(report.sources["remind"].read_later, 1);
        Ok(())
    }

    #[test]
    fn scan_format_migration_rebuilds_derived_tables_and_preserves_injections() -> Result<()> {
        let wiki = Wiki::new()?;
        let dirs = vec![wiki.root.join("transcripts")];
        let project = dirs[0].join("project");
        fs::create_dir_all(&project)?;
        let path = project.join("main.jsonl");
        let command = format!(
            "cat >{}/pages/knowledge/target.md <<'EOF'\ncat /wiki/pages/knowledge/fake.md;\nEOF",
            wiki.root.display()
        );
        let text = format!("{}\n", line("same", "Bash", json!({"command": command})));
        fs::write(&path, &text)?;
        let legacy = Connection::open(wiki.root.join("usage.sqlite"))?;
        legacy.execute_batch("create table tools(tool_use_id text primary key, session_id text, kind text, at integer);
            create table accesses(tool_use_id text, path text, primary key(tool_use_id, path));
            create table transcripts(path text primary key, offset integer, size integer, mtime integer, fingerprint text);
            create table sessions(session_id text primary key, first_at integer, last_at integer);
            create table injections(session_id text, path text, source text, at integer);
            create table state(key text primary key, value integer);
            insert into state values ('scanned', 1);
            insert into tools values ('same', 'wrong-session', 'read', 1);
            insert into accesses values ('same', 'pages/knowledge/target.md');
            insert into sessions values ('wrong-session', 1, 1);
            insert into injections values ('parent', 'pages/knowledge/target.md', 'remind', 1);")?;
        let (mtime, size) = index::stamp(&path.metadata()?);
        legacy.execute(
            "insert into transcripts values (?, ?, ?, ?, '')",
            params![path.to_str(), size, size, mtime],
        )?;
        drop(legacy);
        log(
            &wiki.root,
            "new-session",
            "context",
            &["pages/knowledge/target.md".into()],
        );
        let mut db = open(&wiki.root)?;
        assert!(!scanned(&db)?);
        assert_eq!(
            db.query_row("select kind from tools", [], |r| r.get::<_, String>(0))?,
            "read"
        );
        scan(&wiki.root, &mut db, &dirs)?;
        assert!(scanned(&db)?);
        assert_eq!(
            db.query_row("select count(*) from injections", [], |r| r
                .get::<_, i64>(0))?,
            2
        );
        assert_eq!(
            db.query_row("select kind from accesses", [], |r| r.get::<_, String>(0))?,
            "write"
        );
        assert!(read_pages(&db, i64::MIN)?.is_empty());
        assert_eq!(
            db.query_row("select session_id from sessions", [], |r| r
                .get::<_, String>(0))?,
            "parent"
        );
        assert_eq!(
            db.query_row("select offset from transcripts", [], |r| r.get::<_, i64>(0))?,
            size
        );
        db.execute(
            "insert into tools values ('retained', 'past-session', 1)",
            [],
        )?;
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(
            db.query_row("select count(*) from tools", [], |r| r.get::<_, i64>(0))?,
            2
        );
        Ok(())
    }

    #[test]
    fn scripted_sessions_exclude_prior_accesses_and_hook_injections() -> Result<()> {
        let mut wiki = Wiki::new()?;
        let paths: Vec<_> = ["shared", "authored", "unread", "written"]
            .map(|slug| format!("pages/knowledge/{slug}.md"))
            .into();
        for path in &paths {
            wiki.page(path, "---\ntitle: Test\ncreated: 2020-01-01\n---\nbody")?;
        }
        let dirs = vec![wiki.root.join("transcripts")];
        let project = dirs[0].join("project");
        fs::create_dir_all(&project)?;
        let main = project.join("main.jsonl");
        let mut file = File::create(&main)?;
        for (id, name, path) in [
            ("read-shared", "Read", &paths[0]),
            ("read-authored", "Read", &paths[1]),
            ("write-authored", "Write", &paths[1]),
        ] {
            let mut value = line(id, name, json!({"file_path": wiki.root.join(path)}));
            value["entrypoint"] = "cli".into();
            writeln!(file, "{value}")?;
        }
        log(
            &wiki.root,
            "parent",
            "remind",
            &[paths[0].clone(), paths[2].clone()],
        );
        let mut db = open(&wiki.root)?;
        db.execute("update injections set at=1", [])?;
        scan(&wiki.root, &mut db, &dirs)?;
        let now = 1800000000;
        let expected_usage = per_page(&db, now)?;
        let expected_reads = read_pages(&db, i64::MIN)?;
        let mut expected = aggregate(&db, &wiki.index, i64::MIN, now)?;
        assert_eq!(expected.types["knowledge"].cross_session_read, 1);
        assert_eq!(expected.types["knowledge"].older_than_14d_unread, 2);

        let subagents = project.join("scripted/subagents");
        fs::create_dir_all(&subagents)?;
        let mut agent = File::create(subagents.join("agent.jsonl"))?;
        for (id, name, path) in [
            ("scripted-read-shared", "Read", &paths[0]),
            ("scripted-write-shared", "Write", &paths[0]),
            ("scripted-read-unread", "Read", &paths[2]),
            ("scripted-write-only", "Write", &paths[3]),
        ] {
            let mut value = line(id, name, json!({"file_path": wiki.root.join(path)}));
            value["sessionId"] = "scripted".into();
            value["timestamp"] = "2026-10-01T00:00:00Z".into();
            writeln!(agent, "{value}")?;
        }
        log(&wiki.root, "scripted", "remind", &paths);
        log(&wiki.root, "hook-only", "scripted-source", &paths);
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(read_pages(&db, i64::MIN)?.len(), 3);
        // Markers need neither a timestamp nor a tool call, and may arrive later.
        for _ in 0..2 {
            writeln!(
                agent,
                "{}",
                json!({"sessionId": "scripted", "entrypoint": "sdk-cli"})
            )?;
        }
        writeln!(
            file,
            "{}",
            json!({"sessionId": "hook-only", "entrypoint": "sdk-cli"})
        )?;
        scan(&wiki.root, &mut db, &dirs)?;
        expected.coverage.transcripts = 2;
        expected.coverage.scripted_sessions = 2;
        let expected_json = serde_json::to_value(&expected)?;
        for since in [i64::MIN, 2] {
            let report = aggregate(&db, &wiki.index, since, now)?;
            assert_eq!(report.coverage.scripted_sessions, 2);
            assert_eq!(report.coverage.sessions, 1);
            assert_eq!(report.types["knowledge"].ever_read, 2);
            assert_eq!(report.types["knowledge"].cross_session_read, 1);
            assert!(!report.sources.contains_key("scripted-source"));
        }
        assert_eq!(
            serde_json::to_value(aggregate(&db, &wiki.index, i64::MIN, now)?)?,
            expected_json
        );
        assert_eq!(per_page(&db, now)?, expected_usage);
        assert_eq!(read_pages(&db, i64::MIN)?, expected_reads);
        assert_eq!(
            db.query_row("select count(*) from injections", [], |r| r
                .get::<_, i64>(0))?,
            10
        );

        // The previous scan format rebuilds once, without losing hook injections.
        db.execute(
            "update state set value=? where key='scan_format'",
            [SCAN_FORMAT - 1],
        )?;
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(per_page(&db, now)?, expected_usage);
        assert_eq!(
            serde_json::to_value(aggregate(&db, &wiki.index, i64::MIN, now)?)?,
            expected_json
        );
        drop(db);
        fs::remove_dir_all(&project)?;
        let mut db = open(&wiki.root)?;
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(
            serde_json::to_value(aggregate(&db, &wiki.index, i64::MIN, now)?)?,
            expected_json
        );
        assert_eq!(per_page(&db, now)?, expected_usage);
        Ok(())
    }

    #[test]
    fn incremental_scan_deduplicates_resumes_and_attributes_subagents() -> Result<()> {
        let wiki = Wiki::new()?;
        let dirs = vec![wiki.root.join("transcripts")];
        let project = dirs[0].join("project");
        fs::create_dir_all(project.join("parent/subagents"))?;
        let value = line(
            "same",
            "Read",
            json!({"file_path":wiki.root.join("pages/knowledge/test.md")}),
        );
        let main = project.join("main.jsonl");
        let bytes = format!("{value}\n");
        fs::write(&main, &bytes)?;
        fs::write(project.join("resume.jsonl"), &bytes)?;
        fs::write(
            project.join("parent/subagents/agent.jsonl"),
            format!(
                "{}\n",
                line(
                    "agent",
                    "Edit",
                    json!({"file_path":wiki.root.join("pages/knowledge/test.md")})
                )
            ),
        )?;
        let mut db = open(&wiki.root)?;
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(
            db.query_row("select count(*) from tools", [], |r| r.get::<_, i64>(0))?,
            2
        );
        assert_eq!(
            db.query_row("select count(*) from sessions", [], |r| r.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            db.query_row(
                "select session_id from tools where tool_use_id='agent'",
                [],
                |r| r.get::<_, String>(0)
            )?,
            "parent"
        );
        let next = line(
            "next",
            "Read",
            json!({"file_path":wiki.root.join("pages/knowledge/next.md")}),
        )
        .to_string();
        let mut file = fs::OpenOptions::new().append(true).open(&main)?;
        write!(file, "{next}")?;
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(
            db.query_row(
                "select offset from transcripts where path=?",
                [main.to_str().unwrap()],
                |r| r.get::<_, i64>(0)
            )?,
            bytes.len() as i64
        );
        writeln!(file)?;
        scan(&wiki.root, &mut db, &dirs)?;
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(read_pages(&db, i64::MIN)?.len(), 2);
        let rewritten = bytes.replace("same", "diff");
        fs::write(&main, rewritten)?;
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(
            db.query_row("select count(*) from tools", [], |r| r.get::<_, i64>(0))?,
            4
        );
        fs::write(&main, bytes.replace("same", "last"))?;
        scan(&wiki.root, &mut db, &dirs)?;
        assert_eq!(
            db.query_row("select count(*) from tools", [], |r| r.get::<_, i64>(0))?,
            5
        );
        Ok(())
    }

    #[test]
    fn stats_include_all_sources_in_the_window_and_zero_known_sources() -> Result<()> {
        let wiki = Wiki::new()?;
        let db = open(&wiki.root)?;
        db.execute_batch(
            "insert into injections values
            ('s','pages/knowledge/one.md','subagent',10),
            ('s','pages/knowledge/one.md','subagent',11),
            ('s','pages/knowledge/two.md','subagent',10),
            ('s','pages/knowledge/one.md','skill-prompt',10),
            ('s','pages/knowledge/one.md','custom-source',10),
            ('s','pages/knowledge/one.md','expired-source',9);
            insert into tools values ('r','s',12);
            insert into accesses values ('r','pages/knowledge/one.md','read');",
        )?;
        let report = aggregate(&db, &wiki.index, 10, 1800000000)?;
        let sources = &report.sources;
        assert_eq!(
            sources.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "context",
                "custom-source",
                "recall-failure",
                "recall-skill",
                "remind",
                "skill-prompt",
                "subagent"
            ]
        );
        assert_eq!(
            sources["subagent"],
            SourceStats {
                injections: 2,
                rate: 0.5,
                read_later: 1
            }
        );
        assert_eq!(
            sources["skill-prompt"],
            SourceStats {
                injections: 1,
                rate: 1.0,
                read_later: 1
            }
        );
        assert_eq!(sources["custom-source"], sources["skill-prompt"]);
        for source in ["context", "recall-failure", "recall-skill", "remind"] {
            assert_eq!(
                sources[source],
                SourceStats {
                    injections: 0,
                    rate: 0.0,
                    read_later: 0
                }
            );
        }
        let recent = aggregate(&db, &wiki.index, 12, 1800000000)?;
        assert_eq!(recent.sources.len(), 4);
        Ok(())
    }

    #[test]
    fn stats_count_distinct_injections_later_reads_and_nonwriting_sessions() -> Result<()> {
        let mut wiki = Wiki::new()?;
        for slug in ["one", "two", "never"] {
            wiki.page(
                &format!("pages/knowledge/{slug}.md"),
                "---\ntitle: Test\ncreated: 2020-01-01\n---\nbody",
            )?;
        }
        let db = open(&wiki.root)?;
        db.execute_batch("insert into injections values ('author','pages/knowledge/one.md','remind',10), ('author','pages/knowledge/one.md','remind',11), ('reader','pages/knowledge/two.md','remind',10), ('reader','pages/knowledge/never.md','context',10);
            insert into tools values ('w','author',30), ('r','author',20), ('r2','reader',9), ('r3','other',25);
            insert into accesses values ('w','pages/knowledge/one.md','write'), ('r','pages/knowledge/one.md','read'), ('r2','pages/knowledge/two.md','read'), ('r3','pages/knowledge/two.md','read');")?;
        let report = aggregate(&db, &wiki.index, i64::MIN, 1800000000)?;
        assert_eq!(report.sources["remind"].injections, 2);
        assert_eq!(report.sources["remind"].read_later, 1);
        assert_eq!(report.sources["remind"].rate, 0.5);
        assert_eq!(report.types["knowledge"].ever_read, 2);
        assert_eq!(report.types["knowledge"].cross_session_read, 1);
        assert_eq!(report.types["knowledge"].older_than_14d_unread, 1);
        assert_eq!(report.injected_without_read.len(), 2);
        let recent = aggregate(&db, &wiki.index, 21, 1800000000)?;
        assert_eq!(recent.sources["remind"].injections, 0);
        assert_eq!(recent.types["knowledge"].ever_read, 1);
        assert_eq!(recent.types["knowledge"].older_than_14d_unread, 1);
        Ok(())
    }
}

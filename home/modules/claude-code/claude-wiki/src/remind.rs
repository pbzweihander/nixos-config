use crate::sessions::complete_lines;
use anyhow::Result;
use clap::Args as ClapArgs;
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::Value;
use std::{fs::File, io, sync::LazyLock};

#[derive(ClapArgs)]
pub struct Args {
    #[arg(long, env = "CLAUDE_WIKI_REMIND_INTERVAL", default_value_t = 15)]
    pub interval: u64,
}

#[derive(Default, Deserialize)]
struct Hook {
    session_id: Option<String>,
    transcript_path: Option<String>,
    prompt: Option<String>,
}

/// Sessions write to the wiki far more often than they search it, so the hook runs one
/// search of its own on the prompt. Only pages that clearly match are named, and only
/// their paths: the point is to make the session aware a page exists, not to answer for
/// it. A page must contain at least this many of the prompt's terms; a bm25 cutoff was
/// tried first, but its scale moves with the size of the wiki.
const RELATED_MIN_TERMS: usize = 2;
const RELATED_CANDIDATES: i64 = 8;
const RELATED_PAGES: usize = 3;

/// The wiki is English while prompts are usually Korean, so Korean words would only
/// match noise. Identifiers and English terms carry the topic; two of them are the
/// least that says anything.
fn prompt_terms(prompt: &str) -> Vec<String> {
    const STOP: [&str; 34] = [
        "the", "and", "for", "with", "that", "this", "from", "what", "why", "how", "are", "was",
        "not", "but", "you", "your", "can", "does", "did", "has", "have", "been", "when", "which",
        "into", "about", "them", "they", "its", "also", "more", "than", "then", "there",
    ];
    static TERM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[A-Za-z][A-Za-z0-9_.-]{2,}").unwrap());
    let mut terms: Vec<String> = vec![];
    for m in TERM.find_iter(prompt) {
        let term = m.as_str().to_lowercase();
        if !STOP.contains(&term.as_str()) && !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}

fn related(db: &Connection, prompt: &str) -> Result<Vec<String>> {
    // Hook and slash-command wrappers are not questions about the wiki.
    if prompt.trim_start().starts_with('<') {
        return Ok(vec![]);
    }
    let terms = prompt_terms(prompt);
    if terms.len() < 2 {
        return Ok(vec![]);
    }
    let query = crate::search::fts_query(&terms);
    if query.is_empty() {
        return Ok(vec![]);
    }
    let candidates: Vec<(String, String)> = db
        .prepare(
            // History entries are a log of one task; knowledge, project and follow-up
            // pages are what a later session can reuse, so history ranks lower here.
            "select fts.path, fts.title || ' ' || fts.tags || ' ' || fts.body
             from fts join files on files.id = fts.rowid
             where fts match ? and files.blocked = ''
             order by bm25(fts, 0,0,0,0,10,5,3,1)
                      * (case fts.type when 'history' then 0.8 else 1 end) limit ?",
        )?
        .query_map(params![query, RELATED_CANDIDATES], |r| {
            Ok((r.get(0)?, r.get::<_, String>(1)?.to_lowercase()))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(candidates
        .into_iter()
        .filter(|(_, text)| {
            terms.iter().filter(|term| text.contains(*term)).count() >= RELATED_MIN_TERMS
        })
        .map(|(path, _)| {
            path.trim_start_matches("pages/")
                .trim_end_matches(".md")
                .to_owned()
        })
        .take(RELATED_PAGES)
        .collect())
}

fn ran_sync(line: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return false;
    };
    v["type"] == "assistant"
        && v["message"]["content"].as_array().is_some_and(|parts| {
            parts.iter().any(|p| {
                p["type"] == "tool_use"
                    && p["input"]["command"]
                        .as_str()
                        .is_some_and(|s| s.contains("claude-wiki sync"))
            })
        })
}

fn update(db: &Connection, hook: Hook, interval: u64) -> Result<Option<String>> {
    let id = hook.session_id.unwrap_or_default();
    let (mut prompts, mut offset): (i64, i64) = db
        .query_row(
            "select prompts, offset from remind_state where session_id = ?",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, 0));
    if let Some(path) = hook.transcript_path {
        match File::open(path) {
            Ok(file) => {
                if file.metadata()?.len() < offset as u64 {
                    offset = 0;
                }
                offset = i64::try_from(complete_lines(file, offset as u64, |line| {
                    if ran_sync(line) {
                        prompts = 0;
                    }
                    Ok(())
                })?)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("claude-wiki: cannot read reminder transcript: {e}"),
        }
    }
    prompts = prompts.saturating_add(1);
    let message = if prompts as u64 >= interval {
        let message = format!("claude-wiki: {prompts} prompts since this session last ran `claude-wiki sync`. If this session has produced something another session would need (a root cause and its fix, a tool or environment quirk, a decision and its reason, out-of-scope work for later), record it in the wiki now (load the claude-wiki skill); otherwise continue and do not reply to this note.");
        prompts = 0;
        Some(message)
    } else {
        None
    };
    db.execute("insert into remind_state(session_id, prompts, offset) values (?,?,?) on conflict(session_id) do update set prompts=excluded.prompts, offset=excluded.offset", params![id, prompts, offset])?;
    Ok(message)
}

pub fn run(db: &Connection, args: &Args) -> Result<()> {
    let input = io::read_to_string(io::stdin())?;
    let hook = if input.trim().is_empty() {
        Hook::default()
    } else {
        serde_json::from_str(&input)?
    };
    let pages = related(db, hook.prompt.as_deref().unwrap_or_default())?;
    if !pages.is_empty() {
        println!(
            "claude-wiki: pages that may already cover this: {}. Read one with `claude-wiki search <terms>` or the Read tool if it is relevant; otherwise ignore this note and do not reply to it.",
            pages.join(", ")
        );
    }
    if let Some(message) = update(db, hook, args.interval)? {
        println!("{message}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_assistant_tool_commands_reset() {
        assert!(ran_sync(br#"{"type":"assistant","message":{"content":[{"type":"tool_use","input":{"command":"claude-wiki sync pages/knowledge/x.md"}}]}}"#));
        assert!(!ran_sync(br#"{"type":"assistant","message":{"content":[{"type":"text","text":"claude-wiki sync"}]}}"#));
        assert!(!ran_sync(b"bad json"));
    }
    #[test]
    fn prompt_terms_keep_identifiers_and_drop_korean_and_stopwords() {
        assert_eq!(
            prompt_terms("쿼리 파드 restart 하면 cache verify 가 왜 slow 한가 the and"),
            ["restart", "cache", "verify", "slow"]
        );
        assert_eq!(
            prompt_terms("usemap multi asmr-v2 merge usemap"),
            ["usemap", "multi", "asmr-v2", "merge"]
        );
        // Two characters is too short to be a useful term, Korean carries none.
        assert!(prompt_terms("이 코드 좀 고쳐줘 go 는 왜").is_empty());
    }

    #[test]
    fn related_needs_two_terms_and_a_clear_match() -> Result<()> {
        let db = Connection::open_in_memory()?;
        db.execute_batch(
            "create table files(id integer primary key, path text, blocked text not null default '');
             create virtual table fts using fts5(path unindexed, type unindexed, created unindexed,
                 status unindexed, title, tags, project, body,
                 tokenize = 'porter unicode61 remove_diacritics 2');",
        )?;
        for (id, path, kind, title, body) in [
            (
                1,
                "pages/knowledge/cache-restart.md",
                "knowledge",
                "restart re-verifies the cache",
                "restart cache verify slow",
            ),
            (
                2,
                "pages/history/2026-09-18-cache.md",
                "history",
                "restart re-verifies the cache",
                "restart cache verify slow",
            ),
        ] {
            db.execute(
                "insert into files(id, path) values (?, ?)",
                params![id, path],
            )?;
            db.execute("insert into fts(rowid, path, type, created, status, title, tags, project, body) values (?,?,?,'','',?,'','',?)",
                params![id, path, kind, title, body])?;
        }
        assert_eq!(
            related(&db, "restart 하면 cache verify 때문에 slow 한 이유는")?,
            ["knowledge/cache-restart", "history/2026-09-18-cache"]
        );
        assert!(related(&db, "cache 하나만 물어볼게")?.is_empty());
        assert!(related(
            &db,
            "<command-name>/config</command-name> restart cache verify slow"
        )?
        .is_empty());
        Ok(())
    }

    #[test]
    fn fifteenth_prompt_and_session_isolation() -> Result<()> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("create table remind_state(session_id text primary key, prompts integer, offset integer)")?;
        for i in 1..=16 {
            assert_eq!(update(&db, Hook::default(), 15)?.is_some(), i == 15);
        }
        assert!(update(
            &db,
            Hook {
                session_id: Some("other".into()),
                ..Hook::default()
            },
            15
        )?
        .is_none());
        Ok(())
    }
}

use crate::sessions::complete_lines;
use anyhow::Result;
use clap::Args as ClapArgs;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashSet, fs::File, io, path::Path};

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
        let message = format!(
            "claude-wiki: {prompts} prompts since this session last ran `claude-wiki sync`. If this session has produced something another session would need (a root cause and its fix, a tool or environment quirk, a decision and its reason, out-of-scope work for later), record it in the wiki now (load the claude-wiki skill); otherwise continue and do not reply to this note."
        );
        prompts = 0;
        Some(message)
    } else {
        None
    };
    db.execute("insert into remind_state(session_id, prompts, offset) values (?,?,?) on conflict(session_id) do update set prompts=excluded.prompts, offset=excluded.offset", params![id, prompts, offset])?;
    Ok(message)
}

fn select(
    db: &Connection,
    hits: Vec<crate::semantic::Hit>,
    seen: &HashSet<String>,
) -> Result<Vec<(String, String)>> {
    let mut selected = vec![];
    let mut paths = HashSet::new();
    for hit in hits {
        let Some(path) = crate::pages::link_path(&hit.path) else {
            continue;
        };
        if seen.contains(&path) || !paths.insert(path.clone()) {
            continue;
        }
        let title: Option<String> = db.query_row(
            "select fts.title from fts join files on files.id=fts.rowid where fts.path=? and fts.type!='history' and files.blocked=''",
            [&path], |r| r.get(0)).optional()?;
        if let Some(title) = title {
            selected.push((path, title));
            if selected.len() == 3 {
                break;
            }
        }
    }
    Ok(selected)
}

pub fn run(root: &Path, db: &Connection, args: &Args) -> Result<()> {
    let input = io::read_to_string(io::stdin())?;
    let hook = if input.trim().is_empty() {
        Hook::default()
    } else {
        serde_json::from_str(&input)?
    };
    let prompt = hook.prompt.as_deref().unwrap_or_default();
    let session = hook.session_id.as_deref().unwrap_or_default();
    let seen = crate::recall::transcript_seen(db, session, hook.transcript_path.as_deref())?;
    let selected = if prompt.trim_start().starts_with('<') || prompt.trim().is_empty() {
        vec![]
    } else {
        select(db, crate::semantic::related(prompt), &seen)?
    };
    let paths: Vec<_> = selected.iter().map(|(path, _)| path.clone()).collect();
    crate::recall::mark_seen(db, session, &paths)?;
    if !selected.is_empty() {
        let root = root.canonicalize()?;
        let pages = selected
            .iter()
            .map(|(path, title)| {
                format!(
                    "{} ({})",
                    root.join(path).display(),
                    title.chars().take(120).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        println!(
            "claude-wiki: pages that may already cover this: {pages}. Read one if it bears on the request; otherwise ignore this note and do not reply to it."
        );
        crate::usage::log(&root, session, "remind", &paths);
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
    fn filtering_skips_history_seen_blocked_and_caps_after_filtering() -> Result<()> {
        let mut wiki = crate::usage::tests::Wiki::new()?;
        for name in [
            "history/past",
            "knowledge/seen",
            "knowledge/blocked",
            "knowledge/a",
            "knowledge/b",
            "knowledge/c",
            "knowledge/d",
        ] {
            wiki.page(
                &format!("pages/{name}.md"),
                "---\ntitle: Test\ncreated: 2020-01-01\n---\nbody",
            )?;
        }
        wiki.index.execute(
            "update files set blocked='secret' where path='pages/knowledge/blocked.md'",
            [],
        )?;
        let hits = [
            "history/past",
            "knowledge/seen",
            "knowledge/blocked",
            "knowledge/a",
            "knowledge/b",
            "knowledge/c",
            "knowledge/d",
        ]
        .into_iter()
        .map(|path| crate::semantic::Hit {
            path: path.into(),
            section: None,
        })
        .collect();
        let selected = select(
            &wiki.index,
            hits,
            &HashSet::from(["pages/knowledge/seen.md".into()]),
        )?;
        assert_eq!(
            selected.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(),
            [
                "pages/knowledge/a.md",
                "pages/knowledge/b.md",
                "pages/knowledge/c.md"
            ]
        );
        Ok(())
    }
    #[test]
    fn only_assistant_tool_commands_reset() {
        assert!(ran_sync(br#"{"type":"assistant","message":{"content":[{"type":"tool_use","input":{"command":"claude-wiki sync pages/knowledge/x.md"}}]}}"#));
        assert!(!ran_sync(br#"{"type":"assistant","message":{"content":[{"type":"text","text":"claude-wiki sync"}]}}"#));
        assert!(!ran_sync(b"bad json"));
    }
    #[test]
    fn fifteenth_prompt_and_session_isolation() -> Result<()> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("create table remind_state(session_id text primary key, prompts integer, offset integer)")?;
        for i in 1..=16 {
            assert_eq!(update(&db, Hook::default(), 15)?.is_some(), i == 15);
        }
        assert!(
            update(
                &db,
                Hook {
                    session_id: Some("other".into()),
                    ..Hook::default()
                },
                15
            )?
            .is_none()
        );
        Ok(())
    }
}

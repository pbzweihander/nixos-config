use crate::hook_payload::{Harness, Hook, ran_sync};
use anyhow::Result;
use clap::Args as ClapArgs;
use rusqlite::{Connection, OptionalExtension, params};
use std::{collections::HashSet, path::Path};

#[derive(ClapArgs)]
pub struct Args {
    #[arg(long, value_enum, default_value = "claude")]
    pub harness: Harness,
    #[arg(long, env = crate::compat::env_name("AGENT_WIKI_REMIND_INTERVAL", "CLAUDE_WIKI_REMIND_INTERVAL"), default_value_t = 15)]
    pub interval: u64,
    /// Handle PostToolUse, PostToolUseFailure or PreCompact hook JSON
    #[arg(long)]
    pub event: bool,
    /// Failures before a successful commit/test counts as resolved debugging
    #[arg(long, env = crate::compat::env_name("AGENT_WIKI_REMIND_FAILURES", "CLAUDE_WIKI_REMIND_FAILURES"), default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..))]
    pub failures: u32,
}

/// Whether a session gets the periodic `agent-wiki sync` note.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nudge {
    Emit,
    /// Orca bot homes delegate technical work and do not sync every few turns;
    /// the counter still advances and resets so state stays consistent.
    Suppress,
}

fn update(db: &Connection, hook: Hook, interval: u64, nudge: Nudge) -> Result<Option<String>> {
    let tx = crate::state::transaction(db)?;
    let db = &tx;
    let id = hook.session_id.unwrap_or_default();
    let mut synced = false;
    let (mut prompts, mut offset): (i64, i64) = db
        .query_row(
            "select prompts, offset from remind_state where session_id = ?",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, 0));
    offset = i64::try_from(
        crate::hook_io::lines(
            db,
            &format!("remind:{id}"),
            hook.transcript_path.as_deref(),
            offset as u64,
            |line| {
                if ran_sync(line) {
                    prompts = 0;
                    synced = true;
                }
                Ok(())
            },
        )?
        .0,
    )?;
    prompts = prompts.saturating_add(1);
    let message = if prompts as u64 >= interval {
        let count = std::mem::take(&mut prompts);
        (nudge == Nudge::Emit).then(|| format!(
            "agent-wiki: {count} prompts since this session last ran `agent-wiki sync`. If this session has produced something another session would need (a root cause and its fix, a tool or environment quirk, a decision and its reason, out-of-scope work for later), record it in the wiki now (load the agent-wiki skill); otherwise continue and do not reply to this note."
        ))
    } else {
        None
    };
    db.execute("insert into remind_state(session_id, prompts, offset) values (?,?,?) on conflict(session_id) do update set prompts=excluded.prompts, offset=excluded.offset", params![id, prompts, offset])?;
    crate::remind_event::after_prompt(db, &id, synced, message.is_some())?;
    tx.commit()?;
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
    let Some(input) = crate::hook_io::stdin()? else {
        return Ok(());
    };
    let hook = if input.trim().is_empty() {
        Hook::default()
    } else {
        serde_json::from_str(&input)?
    };
    let text = messages(root, db, args, hook)?;
    if !text.is_empty() {
        println!("{}", args.harness.context("UserPromptSubmit", &text));
    }
    Ok(())
}

pub(crate) fn messages(root: &Path, db: &Connection, args: &Args, hook: Hook) -> Result<String> {
    let prompt = hook.prompt.as_deref().unwrap_or_default();
    let session = hook.session_id.as_deref().unwrap_or_default();
    let seen = crate::recall::transcript_seen(root, db, session, hook.transcript_path.as_deref())?;
    let selected = if prompt.trim_start().starts_with('<') || prompt.trim().is_empty() {
        vec![]
    } else {
        select(db, crate::semantic::related(prompt), &seen)?
    };
    let paths: Vec<_> = selected.iter().map(|(path, _)| path.clone()).collect();
    crate::recall::mark_seen(db, session, &paths)?;
    // Unlike the semantic lookup, bulletins are checked for every prompt, including
    // `<`-wrapped ones such as Orca's `<USER_REQUEST>`: each change is announced once.
    let mut messages: Vec<_> = crate::bulletin::hook(
        root,
        db,
        hook.cwd.as_deref(),
        session,
        crate::bulletin::MAX_CHARS,
    )
    .into_iter()
    .collect();
    messages.extend(crate::bot_guard::prompt(root, &hook));
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
        messages.push(format!(
            "agent-wiki: pages that may already cover this: {pages}. Read one if it bears on the request; otherwise ignore this note and do not reply to it."
        ));
        crate::usage::log(&root, session, "remind", &paths);
    }
    let nudge = if crate::bot_guard::is_bot(hook.cwd.as_deref()) {
        Nudge::Suppress
    } else {
        Nudge::Emit
    };
    if let Some(message) = update(db, hook, args.interval, nudge)? {
        messages.push(message);
    }
    Ok(messages.join("\n"))
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
        assert!(!ran_sync(br#"{"type":"assistant","message":{"content":[{"type":"text","text":"agent-wiki sync"}]}}"#));
        assert!(!ran_sync(b"bad json"));
    }
    #[test]
    fn fifteenth_prompt_and_session_isolation() -> Result<()> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("create table remind_state(session_id text primary key, prompts integer, offset integer)")?;
        for i in 1..=16 {
            assert_eq!(
                update(&db, Hook::default(), 15, Nudge::Emit)?.is_some(),
                i == 15
            );
        }
        assert!(
            update(
                &db,
                Hook {
                    session_id: Some("other".into()),
                    ..Hook::default()
                },
                15,
                Nudge::Emit
            )?
            .is_none()
        );
        for _ in 1..=31 {
            let bot = Hook {
                session_id: Some("bot".into()),
                ..Hook::default()
            };
            assert!(update(&db, bot, 15, Nudge::Suppress)?.is_none());
        }
        // The counter reset at prompts 15 and 30, as it does for an emitted note.
        assert_eq!(
            db.query_row(
                "select prompts from remind_state where session_id='bot'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        Ok(())
    }
}

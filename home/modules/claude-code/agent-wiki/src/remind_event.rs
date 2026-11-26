//! Cheap, provider-neutral hook parsing; see event-reminders.md for the heuristics.
use crate::hook_payload::{Harness, Hook, additional_context};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use std::{io::Read, path::Path};

#[cfg(test)]
use std::fs::{self, OpenOptions};

const MAX_INPUT: u64 = 256 * 1024;

#[derive(Clone, Copy)]
enum Event {
    Commit = 1,
    PullRequest = 2,
    Merge = 4,
    Compact = 8,
    Debugging = 16,
}

#[derive(Default)]
struct State {
    seen: i64,
    failures: u32,
    synced: bool,
}

fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "create table if not exists remind_state(session_id text primary key, prompts integer not null, offset integer not null);
         create table if not exists remind_event_state(session_id text primary key,
             seen integer not null, failures integer not null, synced integer not null);",
    )?;
    Ok(())
}

impl State {
    fn load(db: &Connection, id: &str) -> Result<Self> {
        Ok(db
            .query_row(
                "select seen, failures, synced from remind_event_state where session_id = ?",
                [id],
                |r| {
                    Ok(Self {
                        seen: r.get(0)?,
                        failures: r.get(1)?,
                        synced: r.get(2)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    fn save(&self, db: &Connection, id: &str) -> Result<()> {
        db.execute(
            "insert into remind_event_state values (?, ?, ?, ?) on conflict(session_id) do update set seen=excluded.seen, failures=excluded.failures, synced=excluded.synced",
            params![id, self.seen, self.failures, self.synced],
        )?;
        Ok(())
    }

    fn sync(&mut self) {
        self.synced = true;
        self.failures = 0;
    }
}

// A window ends at the next interval reminder. Event reminders reset the prompt
// counter, but retain seen kinds; a wiki write silences events for this window.
pub(crate) fn after_prompt(db: &Connection, id: &str, synced: bool, fired: bool) -> Result<()> {
    initialize(db)?;
    let mut state = State::load(db, id)?;
    if synced {
        state.sync();
    }
    if fired {
        state.seen = 0;
        state.synced = false;
    }
    state.save(db, id)
}

// This is intentionally not a shell interpreter. Only simple commands connected
// by && are recognized; quotes protect message/argument text from being commands.
fn commands(command: &str) -> Option<Vec<Vec<String>>> {
    let mut commands = vec![];
    let mut words = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut chars = command.trim().chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if q == c => quote = None,
            (Some('\''), c) => word.push(c),
            (_, '`') => return None,
            (_, '$') if chars.peek() == Some(&'(') => return None,
            (_, '\\') => word.push(chars.next()?),
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => quote = Some(c),
            (None, '&') if chars.next() == Some('&') => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                if words.is_empty() {
                    return None;
                }
                commands.push(std::mem::take(&mut words));
            }
            (None, ';' | '|' | '&' | '<' | '>' | '(' | ')' | '\n' | '#') => return None,
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (None, c) => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    if words.is_empty() {
        return None;
    }
    commands.push(words);
    if commands.iter().any(|c| {
        matches!(
            c[0].as_str(),
            "if" | "then" | "for" | "while" | "until" | "!"
        )
    }) {
        return None;
    }
    Some(commands)
}

#[derive(Default)]
struct Actions {
    commit: bool,
    pr: bool,
    merge: bool,
    test: bool,
    sync: bool,
}

fn actions(command: &str) -> Actions {
    let mut result = Actions::default();
    for words in commands(command).unwrap_or_default() {
        let mut words: Vec<_> = words.iter().map(String::as_str).collect();
        while words.first().is_some_and(|w| {
            *w == "env"
                || *w == "command"
                || w.split_once('=').is_some_and(|(key, _)| {
                    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
        }) {
            words.remove(0);
        }
        let Some(program) = words.first().copied() else {
            continue;
        };
        let program = program.rsplit('/').next().unwrap_or(program);
        let mut args = &words[1..];
        if matches!(program, "git" | "gh") {
            while let Some(arg) = args.first() {
                if matches!(
                    *arg,
                    "-C" | "-c" | "--git-dir" | "--work-tree" | "-R" | "--repo" | "--hostname"
                ) && args.len() >= 2
                {
                    args = &args[2..];
                } else if arg.starts_with("--git-dir=")
                    || arg.starts_with("--work-tree=")
                    || arg.starts_with("--repo=")
                {
                    args = &args[1..];
                } else {
                    break;
                }
            }
        }
        if args
            .iter()
            .any(|a| matches!(*a, "--help" | "-h" | "--dry-run" | "--no-run"))
        {
            continue;
        }
        match (program, args) {
            ("git", ["commit", ..]) => result.commit = true,
            ("gh", ["pr", "create", ..]) => result.pr = true,
            ("gh", ["pr", "merge", ..]) if !args.contains(&"--auto") => result.merge = true,
            ("agent-wiki" | "claude-wiki", ["sync", ..]) => result.sync = true,
            ("cargo", ["test", ..]) => result.test = true,
            ("cargo", [toolchain, "test", ..]) if toolchain.starts_with('+') => result.test = true,
            ("go", ["test", ..]) if !args.contains(&"-c") => result.test = true,
            ("pytest" | "pytest-3" | "ctest", _) => result.test = true,
            ("python" | "python3", ["-m", "pytest" | "unittest", ..]) => result.test = true,
            ("npm" | "pnpm" | "yarn" | "bun", ["run", name, ..] | [name, ..])
                if *name == "test" || name.starts_with("test:") =>
            {
                result.test = true
            }
            ("make" | "just" | "deno" | "dotnet", ["test", ..]) => result.test = true,
            _ => {}
        }
    }
    result
}

fn successful(hook: &Hook) -> bool {
    let r = &hook.tool_response;
    hook.hook_event_name == "PostToolUse"
        && hook.tool_name == "Bash"
        && r.is_object()
        && !hook.is_interrupt
        && r["interrupted"] != true
        && r["is_error"] != true
        && r["isError"] != true
        && r["success"] != false
        && (r["error"].is_null() || r["error"] == "")
        && hook.tool_input["run_in_background"] != true
        && r["backgroundTaskId"].is_null()
        && r["task_id"].is_null()
        && r["status"] != "running"
        && ["exit_code", "exitCode"]
            .iter()
            .all(|key| r.get(key).is_none_or(|code| code.as_i64() == Some(0)))
}

// Bound work on a newly enabled hook or a large tool result. Partial JSONL lines
// are retried; if we cannot catch up, save progress and omit this event's nudge.
pub(crate) fn decide(db: &Connection, hook: &Hook, failures: u32) -> Result<Option<String>> {
    if hook
        .session_id
        .as_deref()
        .unwrap_or_default()
        .trim()
        .is_empty()
        || !matches!(
            hook.hook_event_name.as_str(),
            "PostToolUse" | "PostToolUseFailure" | "PreCompact"
        )
    {
        return Ok(None);
    }
    initialize(db)?;
    let tx = crate::state::transaction(db)?;
    let id = hook.session_id.as_deref().unwrap_or_default();
    let mut state = State::load(&tx, id)?;
    let (mut prompts, offset): (i64, i64) = tx
        .query_row(
            "select prompts, offset from remind_state where session_id = ?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or_default();
    let mut synced = false;
    let (offset, caught_up) = crate::hook_io::lines(
        &tx,
        &format!("remind:{id}"),
        hook.transcript_path.as_deref(),
        offset.try_into()?,
        |line| {
            synced |= crate::hook_payload::ran_sync(line);
            Ok(())
        },
    )?;
    let acts = if successful(hook) {
        actions(hook.tool_input["command"].as_str().unwrap_or_default())
    } else {
        Actions::default()
    };
    if synced || acts.sync {
        state.sync();
        prompts = 0;
    }
    if hook.hook_event_name == "PostToolUseFailure" && !hook.is_interrupt {
        state.failures = state.failures.saturating_add(1);
    }
    let resolved = state.failures >= failures && (acts.commit || acts.test);
    let mut events = vec![];
    if resolved {
        events.push((
            Event::Debugging,
            "you just resolved a debugging streak with a successful commit or test run",
        ));
    }
    if acts.commit {
        events.push((Event::Commit, "you just committed changes"));
    }
    if acts.pr {
        events.push((Event::PullRequest, "you just created a pull request"));
    }
    if acts.merge {
        events.push((Event::Merge, "you just merged a pull request"));
    }
    if hook.hook_event_name == "PreCompact" {
        events.push((
            Event::Compact,
            "context compaction is about to discard session details",
        ));
    }
    let mut reasons = vec![];
    if caught_up && !state.synced {
        for (event, reason) in events {
            if state.seen & event as i64 == 0 {
                state.seen |= event as i64;
                reasons.push(reason);
            }
        }
    }
    if acts.commit || acts.test {
        state.failures = 0;
    }
    let message = if reasons.is_empty() {
        None
    } else {
        prompts = 0;
        Some(format!(
            "agent-wiki: {}. If there is a durable finding, decision, or fix another session would need, record it in the wiki now (load the agent-wiki skill); otherwise continue and do not reply to this note.{}",
            reasons.join("; "),
            if hook.hook_event_name == "PreCompact" {
                " Preserve any unwritten finding in the compaction summary so it can be recorded afterward."
            } else {
                ""
            },
        ))
    };
    state.save(&tx, id)?;
    tx.execute("insert into remind_state values (?, ?, ?) on conflict(session_id) do update set prompts=excluded.prompts, offset=excluded.offset", params![id, prompts, i64::try_from(offset)?])?;
    tx.commit()?;
    Ok(message)
}

fn parse(input: impl Read) -> Result<Hook> {
    let mut bytes = vec![];
    input.take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_INPUT,
        "event hook input too large"
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(value.is_object(), "event hook input must be an object");
    Ok(serde_json::from_value(value)?)
}

fn output(hook: &Hook, message: &str) -> String {
    if hook.hook_event_name == "PreCompact" {
        // PreCompact stdout is a compaction instruction, not additionalContext.
        message.into()
    } else {
        additional_context(&hook.hook_event_name, message).to_string()
    }
}

pub(crate) fn run(root: &Path, args: &crate::remind::Args) -> Result<()> {
    let mut hook = parse(crate::hook_io::stdin()?.as_bytes())?;
    if args.harness == Harness::Codex {
        let Some(code) = hook.exit_code() else {
            return Ok(());
        };
        hook.normalize_shell();
        if code != 0 {
            hook.hook_event_name = "PostToolUseFailure".into();
        }
        hook.tool_response = json!({"exit_code": code});
    }
    let db = crate::state::open(root)?;
    if let Some(message) = decide(&db, &hook, args.failures)? {
        println!("{}", output(&hook, &message));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::Write;

    fn hook(event: &str, command: &str) -> Hook {
        serde_json::from_value(json!({
            "session_id": "test", "cwd": "/tmp", "hook_event_name": event,
            "tool_name": "Bash", "tool_input": {"command": command},
            "tool_response": {"stdout": "", "stderr": "", "interrupted": false},
            "unknown_provider_field": "ignored",
        }))
        .unwrap()
    }

    #[test]
    fn detects_executed_commands_not_argument_text_or_ambiguous_shell() {
        for command in [
            "git commit -m 'fix it'",
            "git add . && git commit -m \"fix; && message\"",
            "cd '/tmp/my repo' && env KEY=value git -C . -c commit.gpgsign=true commit",
            "/usr/bin/git --git-dir=/tmp/git commit --amend --no-edit",
        ] {
            assert!(actions(command).commit, "{command}");
        }
        for command in [
            "echo 'git commit'",
            "rg 'git commit' README.md",
            "git commit --dry-run",
            "git commit --help",
            "false; git commit",
            "false && git commit || true",
            "if false && git commit",
            "git commit | cat",
            "git commit &",
            "echo $(git commit)",
            "bash -c 'git commit'",
            "git commit >log",
            "git commit\necho ok",
            "git commit -m 'unfinished",
        ] {
            assert!(!actions(command).commit, "{command}");
        }
        assert!(actions("gh -R owner/repo pr create --title 'hello'").pr);
        assert!(actions("gh pr merge 12 --squash").merge);
        assert!(!actions("gh pr merge 12 --auto").merge);
        assert!(!actions("gh pr create --dry-run").pr);
        assert!(actions("agent-wiki sync pages/knowledge/fix.md").sync);
        assert!(actions("claude-wiki sync pages/knowledge/fix.md").sync);
        assert!(!actions("echo 'agent-wiki sync'").sync);
    }

    #[test]
    fn test_command_allowlist_excludes_compile_only_runs() {
        for command in [
            "cargo test --all-targets",
            "cargo +nightly test",
            "go test ./...",
            "pytest -q",
            "python3 -m pytest",
            "python -m unittest discover",
            "npm test",
            "pnpm run test:unit",
            "yarn test",
            "bun test",
            "ctest",
            "make test",
            "just test",
            "dotnet test",
            "deno test",
        ] {
            assert!(actions(command).test, "{command}");
        }
        for command in [
            "cargo test --no-run",
            "go test -c",
            "echo 'pytest'",
            "cargo check",
            "npm run build",
        ] {
            assert!(!actions(command).test, "{command}");
        }
    }

    #[test]
    fn success_uses_hook_event_and_rejects_errors_interruptions_and_background() {
        let mut h = hook("PostToolUse", "git commit");
        // Claude's real Bash shape has no exit code; warnings on stderr are OK.
        h.tool_response["stderr"] = json!("warning: CRLF");
        assert!(successful(&h));
        for response in [
            json!({"exit_code": 1}),
            json!({"exitCode": 128}),
            json!({"exit_code": null}),
            json!({"interrupted": true}),
            json!({"is_error": true}),
            json!({"isError": true}),
            json!({"success": false}),
            json!({"error": "oops"}),
            json!({"backgroundTaskId": "abc"}),
            json!({"status": "running"}),
            json!("Exit code 1"),
            Value::Null,
        ] {
            h.tool_response = response;
            assert!(!successful(&h), "{}", h.tool_response);
        }
        h.tool_response = json!({"exit_code": 0});
        assert!(successful(&h));
        h.tool_input["run_in_background"] = json!(true);
        assert!(!successful(&h));
        assert!(!successful(&hook("PostToolUseFailure", "git commit")));
    }

    #[test]
    fn dedupe_per_kind_session_and_window_resets_interval_counter() -> Result<()> {
        let db = Connection::open_in_memory()?;
        initialize(&db)?;
        db.execute("insert into remind_state values ('test', 14, 0)", [])?;
        let mut h = hook("PostToolUse", "git commit");
        assert!(decide(&db, &h, 3)?.unwrap().contains("you just committed"));
        let prompts: i64 = db.query_row("select prompts from remind_state", [], |r| r.get(0))?;
        assert_eq!(prompts, 0);
        assert!(decide(&db, &h, 3)?.is_none());
        h.tool_input["command"] = json!("gh pr create");
        assert!(
            decide(&db, &h, 3)?
                .unwrap()
                .contains("created a pull request")
        );
        assert!(decide(&db, &h, 3)?.is_none());
        h.tool_input["command"] = json!("gh pr merge");
        assert!(
            decide(&db, &h, 3)?
                .unwrap()
                .contains("merged a pull request")
        );
        h.session_id = Some("other".into());
        assert!(decide(&db, &h, 3)?.is_some());
        h.session_id = Some("test".into());
        after_prompt(&db, "test", false, true)?;
        assert!(decide(&db, &h, 3)?.is_some());
        Ok(())
    }

    #[test]
    fn sync_suppresses_events_until_interval_reminder() -> Result<()> {
        let db = Connection::open_in_memory()?;
        let commit = hook("PostToolUse", "git commit");
        assert!(decide(&db, &commit, 3)?.is_some());
        assert!(decide(&db, &hook("PostToolUse", "agent-wiki sync"), 3)?.is_none());
        let compact = hook("PreCompact", "");
        assert!(decide(&db, &compact, 3)?.is_none());
        assert!(decide(&db, &hook("PostToolUse", "gh pr create"), 3)?.is_none());
        // Prompt-side transcript scans also persist suppression for later events.
        after_prompt(&db, "test", false, true)?;
        after_prompt(&db, "test", true, false)?;
        assert!(decide(&db, &compact, 3)?.is_none());
        after_prompt(&db, "test", false, true)?;
        assert!(decide(&db, &compact, 3)?.is_some());
        assert!(decide(&db, &compact, 3)?.is_none());
        Ok(())
    }

    #[test]
    fn failures_resolve_after_threshold_and_other_successes_do_not_clear_them() -> Result<()> {
        let db = Connection::open_in_memory()?;
        let mut failure = hook("PostToolUseFailure", "cargo test");
        failure.tool_name = "Edit".into();
        failure.is_interrupt = true;
        assert!(decide(&db, &failure, 3)?.is_none());
        assert_eq!(State::load(&db, "test")?.failures, 0);
        failure.is_interrupt = false;
        for _ in 0..2 {
            assert!(decide(&db, &failure, 3)?.is_none());
        }
        assert!(decide(&db, &hook("PostToolUse", "cargo test"), 3)?.is_none());
        for _ in 0..3 {
            assert!(decide(&db, &failure, 3)?.is_none());
        }
        assert!(decide(&db, &hook("PostToolUse", "git status"), 3)?.is_none());
        let message = decide(&db, &hook("PostToolUse", "git commit"), 3)?.unwrap();
        assert!(message.contains("resolved a debugging streak"));
        assert!(message.contains("you just committed"));
        assert_eq!(State::load(&db, "test")?.failures, 0);
        for _ in 0..3 {
            decide(&db, &failure, 3)?;
        }
        assert!(decide(&db, &hook("PostToolUse", "pytest"), 3)?.is_none());
        after_prompt(&db, "test", false, true)?;
        for _ in 0..2 {
            decide(&db, &failure, 2)?;
        }
        assert!(
            decide(&db, &hook("PostToolUse", "pytest"), 2)?
                .unwrap()
                .contains("resolved a debugging streak")
        );
        Ok(())
    }

    #[test]
    fn transcript_sync_cursor_partial_lines_and_truncation() -> Result<()> {
        let wiki = crate::usage::tests::Wiki::new()?;
        let path = wiki.root.join("event-transcript.jsonl");
        let sync = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","input":{"command":"agent-wiki sync"}}]}}"#;
        fs::write(&path, sync)?;
        let h = Hook {
            transcript_path: Some(path.to_string_lossy().into_owned()),
            ..hook("PostToolUse", "git commit")
        };
        assert!(decide(&wiki.index, &h, 3)?.is_none()); // partial transcript: do not guess
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"\n")?;
        assert!(decide(&wiki.index, &h, 3)?.is_none()); // complete sync suppresses
        assert!(State::load(&wiki.index, "test")?.synced);
        after_prompt(&wiki.index, "test", false, true)?;
        assert!(decide(&wiki.index, &h, 3)?.is_some()); // shared cursor does not replay sync
        fs::write(&path, "{}\n")?;
        assert_eq!(
            crate::hook_io::lines(
                &wiki.index,
                "replace",
                h.transcript_path.as_deref(),
                999,
                |_| Ok(())
            )?,
            (3, true)
        );
        Ok(())
    }

    #[test]
    fn invalid_input_and_database_errors_are_silent_through_hook_boundary() -> Result<()> {
        for input in ["", "bad json", "[]", r#"{"session_id": 42}"#] {
            assert!(parse(input.as_bytes()).is_err());
        }
        assert!(parse(&vec![b' '; MAX_INPUT as usize + 1][..]).is_err());
        let db = Connection::open_in_memory()?;
        assert!(decide(&db, &Hook::default(), 3)?.is_none());
        assert!(decide(&db, &hook("PreToolUse", "git commit"), 3)?.is_none());
        db.execute_batch("pragma query_only = on")?;
        assert!(decide(&db, &hook("PostToolUse", "git commit"), 3).is_err());
        // main.rs maps these errors to exit zero; smoke tests cover that boundary.
        Ok(())
    }

    #[test]
    fn output_uses_the_event_specific_delivery_contract() {
        let h = hook("PostToolUse", "git commit");
        let value: Value = serde_json::from_str(&output(&h, "reminder")).unwrap();
        assert_eq!(value["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert_eq!(value["hookSpecificOutput"]["additionalContext"], "reminder");
        assert_eq!(output(&hook("PreCompact", ""), "reminder"), "reminder");
    }
}

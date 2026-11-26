//! Antigravity's protojson hooks have a distinct wire contract: tool hooks cannot
//! inject context. Stage their notes and deliver them at an invocation boundary.
use crate::{Commands, hook_payload::Hook};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Payload {
    pub conversation_id: String,
    pub workspace_paths: Vec<String>,
    pub transcript_path: Option<String>,
    pub cwd: String,
    pub last_user_input: String,
    pub hook_event_name: String,
    pub invocation_num: Option<u64>,
    pub initial_num_steps: Option<u64>,
    pub step_idx: Option<u64>,
    pub tool_call: ToolCall,
    pub error: Value,
    pub termination_reason: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

impl Payload {
    pub fn cwd(&self) -> &str {
        if self.cwd.is_empty() {
            self.workspace_paths.first().map_or("", String::as_str)
        } else {
            &self.cwd
        }
    }

    pub fn hook(&self) -> Hook {
        let mut hook = Hook {
            session_id: Some(self.conversation_id.clone()),
            transcript_path: self.transcript_path.clone(),
            cwd: Some(self.cwd().into()),
            prompt: Some(self.last_user_input.clone()),
            hook_event_name: self.hook_event_name.clone(),
            tool_name: self.tool_call.name.clone(),
            tool_input: self.tool_call.args.clone(),
            error: self.error.clone(),
            is_interrupt: self.termination_reason == "USER_CANCELED",
            ..Hook::default()
        };
        if hook.tool_name == "run_command" {
            hook.tool_name = "Bash".into();
            hook.tool_input = json!({"command": self.tool_call.args["CommandLine"]});
        }
        hook
    }

    fn tool_key(&self) -> String {
        fingerprint(&json!([
            self.tool_call.id,
            self.step_idx,
            self.tool_call.name,
            self.tool_call.args.to_string(),
        ]))
    }
}

/// Persisted dedup keys must survive toolchain upgrades, so use FNV-1a (64-bit,
/// fixed constants) over the serialized JSON instead of std's unspecified hasher.
fn fingerprint(value: &Value) -> String {
    format!("{:016x}", fnv1a(value.to_string().as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

pub(crate) fn read_input() -> Result<Vec<u8>> {
    let mut bytes = vec![];
    io::stdin().take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "Antigravity hook input too large"
    );
    Ok(bytes)
}

pub(crate) fn read_payload() -> Result<Payload> {
    Ok(serde_json::from_slice(&read_input()?)?)
}

/// The event name of any JSON object input, even one whose other fields do not
/// match `Payload`, so failure paths can still answer PreToolUse with `allow`.
pub(crate) fn event_name(input: &[u8]) -> String {
    serde_json::from_slice::<Value>(input)
        .ok()
        .and_then(|v| v["hookEventName"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub(crate) fn output(event: &str, text: &str) -> Value {
    match event {
        "PreToolUse" => json!({"decision": "allow"}),
        "SessionStart" | "PreInvocation" | "PostInvocation" if !text.trim().is_empty() => {
            json!({"injectSteps": [{"systemMessage": {"systemMessage": text}}]})
        }
        _ => json!({}),
    }
}

fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "create table if not exists antigravity_prompts(session_id text primary key, prompt_key text);
         create table if not exists antigravity_pending(
             session_id text, source text, tool_key text, note text, created integer,
             primary key(session_id,source,tool_key));
         create table if not exists antigravity_tools(
             session_id text, source text, tool_key text, created integer,
             primary key(session_id,source,tool_key));
         delete from antigravity_pending where created < unixepoch()-86400;
         delete from antigravity_tools where created < unixepoch()-604800;",
    )?;
    Ok(())
}

fn new_prompt(db: &Connection, payload: &Payload) -> Result<bool> {
    if payload.hook_event_name != "PreInvocation" || payload.invocation_num != Some(1) {
        return Ok(false);
    }
    // initialNumSteps distinguishes identical text in successive user turns;
    // missing counters fall back to text deduplication. Never persist prompt text.
    let key = fingerprint(&json!([payload.initial_num_steps, payload.last_user_input]));
    let previous: Option<String> = db
        .query_row(
            "select prompt_key from antigravity_prompts where session_id=?",
            [&payload.conversation_id],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_ref() == Some(&key) {
        return Ok(false);
    }
    db.execute(
        "insert into antigravity_prompts values (?,?) on conflict(session_id) do update set prompt_key=excluded.prompt_key",
        params![payload.conversation_id, key],
    )?;
    Ok(true)
}

fn skill(path: &str) -> Option<&str> {
    let path = Path::new(path);
    if path.file_name()? != "SKILL.md" {
        return None;
    }
    let parent = path.parent()?;
    (parent.parent()?.file_name()? == "skills")
        .then(|| parent.file_name()?.to_str())
        .flatten()
}

/// The installed transcript uses snake_case step rows, unlike hook protojson.
/// Only an exact completed stepIdx match is usable as a tool result; do not infer
/// associations from nearby planner calls (parallel tool batches have no IDs).
fn completed_step(payload: &Payload) -> Result<Option<Value>> {
    let (Some(path), Some(step)) = (&payload.transcript_path, payload.step_idx) else {
        return Ok(None);
    };
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        metadata.is_file(),
        "Antigravity transcript must be a regular file"
    );
    let mut file = File::open(path)?;
    let offset = metadata.len().saturating_sub(4 * 1024 * 1024);
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file.take(4 * 1024 * 1024));
    let mut line = vec![];
    if offset > 0 {
        reader.read_until(b'\n', &mut line)?;
    }
    let started = Instant::now();
    let mut found = None;
    while started.elapsed() < Duration::from_millis(100) {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 || !line.ends_with(b"\n") {
            break;
        }
        if let Ok(row) = serde_json::from_slice::<Value>(&line)
            && row["step_index"].as_u64() == Some(step)
            && row["type"] == "GENERIC"
            && matches!(row["status"].as_str(), Some("DONE" | "ERROR"))
        {
            found = Some(row);
        }
    }
    Ok(found)
}

/// Observed run_command results put `Command exited with code N.` in a header
/// (after `Created At`/`Completed At` lines, possibly repeated in a truncation
/// preview) followed by `Command output:` and the raw output. Only the header is
/// searched, so the output body cannot fake an exit status. Content without a
/// `Command output:` line is all header.
pub(crate) fn command_exit(text: &str) -> Option<i64> {
    text.lines()
        .map(str::trim_end)
        .take_while(|line| *line != "Command output:")
        .find_map(|line| {
            let code = line
                .strip_prefix("Command exited with code ")?
                .strip_suffix('.')?;
            let digits = code.strip_prefix('-').unwrap_or(code);
            (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| code.parse().ok())
                .flatten()
        })
}

fn tool_hook(payload: &Payload) -> Hook {
    let mut hook = payload.hook();
    let row = completed_step(payload).ok().flatten();
    let text = row
        .as_ref()
        .and_then(|v| v["content"].as_str())
        .unwrap_or_default();
    let error = payload
        .error
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            row.as_ref()
                .and_then(|v| v["error"].as_str())
                .filter(|s| !s.is_empty())
        });
    let code = (payload.tool_call.name == "run_command")
        .then(|| command_exit(text))
        .flatten();
    hook.tool_response = json!({"output": text, "exit_code": code});
    if error.is_some()
        || code.is_some_and(|c| c != 0)
        || row.as_ref().is_some_and(|v| v["status"] == "ERROR")
    {
        hook.hook_event_name = "PostToolUseFailure".into();
        hook.error = json!([error.unwrap_or_default(), text].join("\n"));
    }
    hook
}

struct Pending {
    rowid: i64,
    note: Option<Value>,
}

/// Select queued notes without deleting them; `commit_drain` removes exactly
/// these rows once the reply is built, so a failure in between keeps them queued
/// and a note staged meanwhile is not removed.
fn pending(db: &Connection, session: &str) -> Result<Vec<Pending>> {
    Ok(db
        .prepare(
            "select rowid, note from antigravity_pending where session_id=? order by created,rowid",
        )?
        .query_map([session], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .map(|row| {
            row.map(|(rowid, note)| Pending {
                rowid,
                // A malformed note can never be delivered: drop it with the rest.
                note: serde_json::from_str(&note).ok(),
            })
        })
        .collect::<rusqlite::Result<_>>()?)
}

fn message(pending: &Pending) -> Option<&str> {
    pending.note.as_ref()?["hookSpecificOutput"]["additionalContext"].as_str()
}

fn commit_drain(root: &Path, db: &Connection, session: &str, drained: &[Pending]) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    {
        let mut delete = tx.prepare("delete from antigravity_pending where rowid=?")?;
        for item in drained {
            delete.execute([item.rowid])?;
        }
    }
    tx.commit()?;
    // Usage counts injections: only notes that were part of the reply.
    for note in drained
        .iter()
        .filter(|item| message(item).is_some())
        .filter_map(|item| item.note.as_ref())
    {
        if let (Some(source), Some(paths)) = (note["source"].as_str(), note["pages"].as_array()) {
            let paths = paths
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            crate::usage::log(root, session, source, &paths);
        }
    }
    Ok(())
}

fn prompt_message(
    root: &Path,
    db: &Connection,
    args: &crate::remind::Args,
    payload: &Payload,
) -> Result<Option<String>> {
    if !new_prompt(db, payload)? {
        return Ok(None);
    }
    let text = crate::remind::messages(root, db, args, payload.hook())?;
    Ok((!text.is_empty()).then_some(text))
}

/// Antigravity kills a hook after its 5 s timeout; keep the lock wait well below it.
const MAX_LOCK_WAIT: Duration = Duration::from_millis(3000);

pub(crate) fn run(command: &Commands, input: &[u8]) -> Result<Value> {
    let payload: Payload = serde_json::from_slice(input)?;
    let empty = output(&payload.hook_event_name, "");
    // This integration never gates tools, even if invoked manually as PreToolUse.
    if payload.hook_event_name == "PreToolUse" || payload.conversation_id.is_empty() {
        return Ok(empty);
    }
    let root = crate::wiki_root();
    Ok(crate::with_hook_lock(&root, MAX_LOCK_WAIT, || {
        run_locked(command, &root, &payload)
    })?
    .unwrap_or(empty))
}

fn run_locked(command: &Commands, root: &Path, payload: &Payload) -> Result<Value> {
    let empty = output(&payload.hook_event_name, "");
    let needs_index = matches!(command, Commands::Context(_) | Commands::Recall(_))
        || matches!(command, Commands::Remind(args) if !args.event && payload.hook_event_name == "PreInvocation" && payload.invocation_num == Some(1));
    let mut db = if needs_index {
        crate::ensure_layout(root)?;
        crate::index::open(root)?
    } else {
        Connection::open(root.join("index.sqlite"))?
    };
    db.busy_timeout(Duration::from_millis(50))?;
    if needs_index {
        crate::index::refresh(root, &mut db)?;
    }
    initialize(&db)?;
    let session = &payload.conversation_id;
    match command {
        Commands::Context(args) if payload.hook_event_name == "SessionStart" => {
            let text =
                crate::context::render(root, &db, args, Some(session), Path::new(payload.cwd()))?;
            Ok(output("SessionStart", &text))
        }
        Commands::Remind(args)
            if !args.event
                && matches!(
                    payload.hook_event_name.as_str(),
                    "PreInvocation" | "PostInvocation"
                ) =>
        {
            let drained = pending(&db, session)?;
            let mut messages: Vec<String> = drained
                .iter()
                .filter_map(message)
                .map(str::to_owned)
                .collect();
            // A prompt reminder failure must not cost the already staged notes.
            match prompt_message(root, &db, args, payload) {
                Ok(Some(text)) => messages.push(text),
                Ok(None) => {}
                Err(e) => eprintln!("agent-wiki: {e:#}"),
            }
            let reply = output(&payload.hook_event_name, &messages.join("\n"));
            commit_drain(root, &db, session, &drained)?;
            Ok(reply)
        }
        Commands::Recall(_) | Commands::Remind(_) if payload.hook_event_name == "PostToolUse" => {
            let source = if matches!(command, Commands::Recall(_)) {
                "recall"
            } else {
                "event"
            };
            let key = payload.tool_key();
            let seen: bool = db.query_row("select exists(select 1 from antigravity_tools where session_id=? and source=? and tool_key=?)", params![session, source, key], |r| r.get(0))?;
            if seen {
                return Ok(empty);
            }
            let mut hook = tool_hook(payload);
            let note = match command {
                Commands::Recall(args) => {
                    if payload.tool_call.name == "read_file"
                        && hook.hook_event_name == "PostToolUse"
                        && let Some(name) = payload.tool_call.args["Path"].as_str().and_then(skill)
                    {
                        hook.hook_event_name = "PreToolUse".into();
                        hook.tool_name = "Skill".into();
                        hook.tool_input = json!({"skill": name});
                    } else if hook.tool_name == "Bash" && hook.hook_event_name == "PostToolUse" {
                        // Reuse the capped, deduplicated diagnostic fallback. Known
                        // successful output retains recall's existing reader guard.
                        if hook.exit_code().is_none() {
                            hook.tool_response = json!({"output": crate::hook_payload::diagnostic_lines(&hook.response_text()).join("\n")});
                        }
                    }
                    crate::recall::decide(root, &db, &hook, args)?
                }
                Commands::Remind(args) => crate::remind_event::decide(&db, &hook, args.failures)?
                    .map(|message| {
                        crate::hook_payload::additional_context("PostToolUse", &message)
                    }),
                _ => unreachable!(),
            };
            if let Some(note) = note {
                db.execute(
                    "insert or ignore into antigravity_pending values (?,?,?,?,unixepoch())",
                    params![session, source, key, note.to_string()],
                )?;
            }
            db.execute(
                "insert or ignore into antigravity_tools values (?,?,?,unixepoch())",
                params![session, source, key],
            )?;
            Ok(empty)
        }
        _ => Ok(empty),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn camelcase_payload_maps_without_confusing_transcript_schema() {
        let payload: Payload = serde_json::from_value(json!({
            "conversationId":"one", "workspacePaths":["/project"],
            "transcriptPath":"/tmp/transcript.jsonl", "lastUserInput":"질문",
            "invocationNum":2, "initialNumSteps":4, "stepIdx":5,
            "hookEventName":"PostToolUse", "terminationReason":"USER_CANCELED",
            "toolCall":{"id":"tool", "name":"run_command", "args":{"CommandLine":"false"}},
            "error":"exit status 1"
        }))
        .unwrap();
        let hook = payload.hook();
        assert_eq!(hook.session_id.as_deref(), Some("one"));
        assert_eq!(hook.cwd.as_deref(), Some("/project"));
        assert_eq!(hook.prompt.as_deref(), Some("질문"));
        assert_eq!(
            hook.transcript_path.as_deref(),
            Some("/tmp/transcript.jsonl")
        );
        assert_eq!(hook.tool_name, "Bash");
        assert_eq!(hook.tool_input["command"], "false");
        assert_eq!(hook.error, "exit status 1");
        assert!(hook.is_interrupt);
        assert_eq!(payload.invocation_num, Some(2));
    }

    #[test]
    fn output_is_nested_inject_steps_or_empty_and_never_continues_stop() {
        for event in ["SessionStart", "PreInvocation", "PostInvocation"] {
            let value = output(event, "줄\n\"인용\"");
            assert_eq!(
                value["injectSteps"][0]["systemMessage"]["systemMessage"],
                "줄\n\"인용\""
            );
            assert_eq!(output(event, ""), json!({}));
        }
        assert_eq!(output("PostToolUse", "ignored"), json!({}));
        assert_eq!(output("Stop", "ignored"), json!({}));
        assert_eq!(output("PreToolUse", ""), json!({"decision":"allow"}));
    }

    #[test]
    fn skill_paths_and_prompt_keys_are_conservative() -> Result<()> {
        assert_eq!(skill("/a/skills/testing/SKILL.md"), Some("testing"));
        assert_eq!(skill("/a/testing/SKILL.md"), None);
        assert_eq!(skill("/a/skills/testing/README.md"), None);
        let db = Connection::open_in_memory()?;
        initialize(&db)?;
        let mut p = Payload {
            conversation_id: "one".into(),
            last_user_input: "same prompt".into(),
            hook_event_name: "PreInvocation".into(),
            invocation_num: Some(1),
            initial_num_steps: Some(0),
            ..Payload::default()
        };
        assert!(new_prompt(&db, &p)?);
        assert!(!new_prompt(&db, &p)?);
        p.invocation_num = Some(2);
        p.last_user_input = "changed on continuation".into();
        assert!(!new_prompt(&db, &p)?);
        p.invocation_num = Some(1);
        p.initial_num_steps = Some(5);
        assert!(new_prompt(&db, &p)?);
        p.hook_event_name = "PostInvocation".into();
        assert!(!new_prompt(&db, &p)?);
        Ok(())
    }
    /// A real-shaped run_command result: header, then arbitrarily long output.
    pub(crate) fn command_result(code: i64, output: &[&str]) -> String {
        format!(
            "Created At: 2026-11-26T22:19:04+09:00\nCompleted At: 2026-11-26T22:19:15+09:00\n\
             Command exited with code {code}.\n\nCommand output:\n{}",
            output.join("\n")
        )
    }

    #[test]
    fn command_exit_reads_only_the_header_before_command_output() {
        let long: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        let long: Vec<&str> = long.iter().map(String::as_str).collect();
        assert_eq!(command_exit(&command_result(1, &long)), Some(1));
        assert_eq!(command_exit(&command_result(0, &long)), Some(0));
        assert_eq!(command_exit(&command_result(-9, &[])), Some(-9));
        // An exit line inside the output body is not the command's status.
        let mut trap = long.clone();
        trap.push("Command exited with code 1.");
        assert_eq!(command_exit(&command_result(0, &trap)), Some(0));
        let headerless = format!("Command output:\n{}", trap.join("\n"));
        assert_eq!(command_exit(&headerless), None);
        // Truncated results repeat the header inside a preview, still before the body.
        let truncated = "Created At: x\nThe tool's output was truncated.\n\nHere is a preview:\n\
                         Created At: x\nCommand exited with code 2.\n\nCommand output:\nbody";
        assert_eq!(command_exit(truncated), Some(2));
        // Without a Command output line, the whole (synthetic) content is header.
        assert_eq!(
            command_exit("error: x\nCommand exited with code 1."),
            Some(1)
        );
        for bad in [
            "Command exited with code +1.",
            "Command exited with code 1",
            "  Command exited with code 1.",
            "Command exited with code .",
            "Command exited with code 1.5.",
        ] {
            assert_eq!(command_exit(bad), None, "{bad}");
        }
    }

    #[test]
    fn fingerprints_are_fixed_fnv1a_values() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(
            fingerprint(&json!("a")),
            format!("{:016x}", fnv1a(b"\"a\""))
        );
        assert_ne!(
            fingerprint(&json!([1, "a"])),
            fingerprint(&json!([null, "a"]))
        );
    }

    #[test]
    fn drain_deletes_only_selected_rows() -> Result<()> {
        let db = Connection::open_in_memory()?;
        initialize(&db)?;
        let stage = |key: &str, text: &str| {
            db.execute(
                "insert into antigravity_pending values ('one','recall',?,?,unixepoch())",
                params![
                    key,
                    crate::hook_payload::additional_context("PostToolUse", text).to_string()
                ],
            )
        };
        stage("a", "first note")?;
        db.execute(
            "insert into antigravity_pending values ('one','recall','bad','not json',unixepoch())",
            [],
        )?;
        let drained = pending(&db, "one")?;
        assert_eq!(
            drained.iter().filter_map(message).collect::<Vec<_>>(),
            ["first note"]
        );
        // Staged after the selection: must survive this drain.
        stage("b", "second note")?;
        commit_drain(Path::new("/nonexistent"), &db, "one", &drained)?;
        let left = pending(&db, "one")?;
        assert_eq!(
            left.iter().filter_map(message).collect::<Vec<_>>(),
            ["second note"]
        );
        Ok(())
    }

    #[test]
    fn partial_payloads_still_identify_pretooluse() {
        assert_eq!(
            event_name(br#"{"hookEventName":"PreToolUse","invocationNum":"bad"}"#),
            "PreToolUse"
        );
        assert!(
            serde_json::from_slice::<Payload>(
                br#"{"hookEventName":"PreToolUse","invocationNum":"bad"}"#
            )
            .is_err()
        );
        assert_eq!(event_name(b"not json"), "");
        assert_eq!(event_name(b"[]"), "");
    }
}

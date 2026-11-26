//! Antigravity's protojson hooks have a distinct wire contract: tool hooks cannot
//! inject context. Stage their notes and deliver them at an invocation boundary.
use crate::{Commands, hook_payload::Hook};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
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
        fingerprint(&(
            &self.tool_call.id,
            self.step_idx,
            &self.tool_call.name,
            self.tool_call.args.to_string(),
        ))
    }
}

fn fingerprint(value: &impl Hash) -> String {
    let mut hash = DefaultHasher::new();
    value.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

pub(crate) fn read_payload() -> Result<Payload> {
    let mut bytes = vec![];
    io::stdin().take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "Antigravity hook input too large"
    );
    Ok(serde_json::from_slice(&bytes)?)
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
    let key = fingerprint(&(payload.initial_num_steps, &payload.last_user_input));
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

pub(crate) fn command_exit(text: &str) -> Option<i64> {
    // Observed run_command footer. Anchored whole line, not arbitrary mentions
    // of an exit code in source files or command output.
    text.lines().rev().take(12).find_map(|line| {
        line.trim()
            .strip_prefix("Command exited with code ")?
            .strip_suffix('.')?
            .parse()
            .ok()
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

fn drain(root: &Path, db: &Connection, session: &str) -> Result<Vec<String>> {
    let tx = db.unchecked_transaction()?;
    let notes = tx
        .prepare("select note from antigravity_pending where session_id=? order by created,rowid")?
        .query_map([session], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut messages = vec![];
    for note in notes {
        let value: Value = serde_json::from_str(&note)?;
        if let Some(message) = value["hookSpecificOutput"]["additionalContext"].as_str() {
            messages.push(message.to_owned());
            if let (Some(source), Some(paths)) =
                (value["source"].as_str(), value["pages"].as_array())
            {
                let paths = paths
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                crate::usage::log(root, session, source, &paths);
            }
        }
    }
    tx.execute(
        "delete from antigravity_pending where session_id=?",
        [session],
    )?;
    tx.commit()?;
    Ok(messages)
}

pub(crate) fn run(command: &Commands) -> Result<Value> {
    let payload = read_payload()?;
    let empty = output(&payload.hook_event_name, "");
    // This integration never gates tools, even if invoked manually as PreToolUse.
    if payload.hook_event_name == "PreToolUse" || payload.conversation_id.is_empty() {
        return Ok(empty);
    }
    let root = crate::wiki_root();
    fs::create_dir_all(&root)?;
    let mut lock = fd_lock::RwLock::new(
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(".lock"))?,
    );
    let Ok(_guard) = lock.try_write() else {
        return Ok(empty);
    };
    let needs_index = matches!(command, Commands::Context(_) | Commands::Recall(_))
        || matches!(command, Commands::Remind(args) if !args.event && payload.hook_event_name == "PreInvocation" && payload.invocation_num == Some(1));
    let mut db = if needs_index {
        crate::ensure_layout(&root)?;
        crate::index::open(&root)?
    } else {
        Connection::open(root.join("index.sqlite"))?
    };
    db.busy_timeout(Duration::from_millis(50))?;
    if needs_index {
        crate::index::refresh(&root, &mut db)?;
    }
    initialize(&db)?;
    let session = &payload.conversation_id;
    match command {
        Commands::Context(args) if payload.hook_event_name == "SessionStart" => {
            let text =
                crate::context::render(&root, &db, args, Some(session), Path::new(payload.cwd()))?;
            Ok(output("SessionStart", &text))
        }
        Commands::Remind(args)
            if !args.event
                && matches!(
                    payload.hook_event_name.as_str(),
                    "PreInvocation" | "PostInvocation"
                ) =>
        {
            let mut messages = drain(&root, &db, session)?;
            if new_prompt(&db, &payload)? {
                let text = crate::remind::messages(&root, &db, args, payload.hook())?;
                if !text.is_empty() {
                    messages.push(text);
                }
            }
            Ok(output(&payload.hook_event_name, &messages.join("\n")))
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
            let mut hook = tool_hook(&payload);
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
                    crate::recall::decide(&root, &db, &hook, args)?
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
mod tests {
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
}

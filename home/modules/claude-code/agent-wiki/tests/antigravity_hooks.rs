use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "wiki-antigravity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("wiki/pages/knowledge")).unwrap();
        fs::create_dir_all(path.join("wiki/pages/projects")).unwrap();
        fs::create_dir_all(path.join("demo")).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(path.join("demo"))
                .status()
                .unwrap()
                .success()
        );
        fs::write(
            path.join("wiki/pages/projects/demo.md"),
            "---\ntitle: 예제 프로젝트\ncreated: 2026-01-01\n---\n프로젝트 요약\n",
        )
        .unwrap();
        fs::write(path.join("wiki/pages/knowledge/socket.md"), "---\ntitle: 소켓 오류\ncreated: 2026-01-01\n---\nerror: cannot establish app-server socket mount isolation\n").unwrap();
        fs::write(
            path.join("wiki/pages/knowledge/skill.md"),
            "---\ntitle: 테스트 스킬\ncreated: 2026-01-01\ntags: [testing]\n---\n스킬 메모\n",
        )
        .unwrap();
        Self(path)
    }

    fn payload(&self, event: &str) -> Value {
        json!({"conversationId": "conversation-one", "workspacePaths": [self.0.join("demo")],
            "transcriptPath": self.0.join("transcript.jsonl"), "hookEventName": event,
            "artifactDirectoryPath": self.0.join("artifacts"), "modelName": "fixture",
            "lastUserInput": "테스트 고쳐 줘", "invocationNum": 1, "initialNumSteps": 1})
    }

    fn run_raw(&self, args: &[&str], input: &str, expect_errors: bool) -> Value {
        self.run_env(args, input, expect_errors, &[])
    }

    fn run_env(
        &self,
        args: &[&str],
        input: &str,
        expect_errors: bool,
        env: &[(&str, &str)],
    ) -> Value {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-wiki"))
            .args(args)
            .args(["--harness", "antigravity"])
            .current_dir(&self.0)
            .env("AGENT_WIKI_DIR", self.0.join("wiki"))
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("AGENT_WIKI_SOCKET", "")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{out:?}");
        if !expect_errors {
            assert!(out.stderr.is_empty(), "{out:?}");
        }
        // Parsing the complete stdout also rejects extra JSON objects/plain text.
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {out:?}"))
    }

    fn run(&self, args: &[&str], input: &Value) -> Value {
        self.run_raw(args, &input.to_string(), false)
    }

    fn lock(&self) -> fd_lock::RwLock<fs::File> {
        wiki_lock(&self.0)
    }

    fn pending(&self) -> i64 {
        rusqlite::Connection::open(self.0.join("wiki/index.sqlite"))
            .unwrap()
            .query_row("select count(*) from antigravity_pending", [], |r| r.get(0))
            .unwrap()
    }

    fn transcript(&self, rows: &[Value]) {
        fs::write(
            self.0.join("transcript.jsonl"),
            rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn wiki_lock(root: &std::path::Path) -> fd_lock::RwLock<fs::File> {
    fd_lock::RwLock::new(
        fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join("wiki/.lock"))
            .unwrap(),
    )
}

fn text(output: &Value) -> &str {
    output["injectSteps"][0]["systemMessage"]["systemMessage"]
        .as_str()
        .unwrap()
}

#[test]
fn context_uses_camelcase_session_and_workspace_fallback_and_logs_injections() {
    let f = Fixture::new();
    let mut p = f.payload("SessionStart");
    let result = f.run(&["context"], &p);
    assert!(text(&result).contains("Current project: demo"));
    assert!(text(&result).contains("프로젝트 요약"));
    let db = rusqlite::Connection::open(f.0.join("wiki/usage.sqlite")).unwrap();
    let session: String = db
        .query_row(
            "select session_id from injections where source='context' limit 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(session, "conversation-one");
    p["cwd"] = json!(f.0);
    assert!(!text(&f.run(&["context"], &p)).contains("Current project: demo"));
}

#[test]
fn invocation_dedup_counts_prompts_not_tool_steps_or_duplicate_hook_delivery() {
    let f = Fixture::new();
    let args = ["remind", "--interval", "2"];
    let mut p = f.payload("PreInvocation");
    assert_eq!(f.run(&args, &p), json!({}));
    assert_eq!(f.run(&args, &p), json!({}));
    for n in 2..20 {
        p["invocationNum"] = json!(n);
        assert_eq!(f.run(&args, &p), json!({}));
        p["hookEventName"] = json!("PostInvocation");
        assert_eq!(f.run(&args, &p), json!({}));
        p["hookEventName"] = json!("PreInvocation");
    }
    // The same prompt in a new user turn is still a new prompt.
    p["invocationNum"] = json!(1);
    p["initialNumSteps"] = json!(20);
    assert!(text(&f.run(&args, &p)).contains("2 prompts"));
    assert_eq!(f.run(&args, &p), json!({}));
    p["conversationId"] = json!("another-conversation");
    assert_eq!(f.run(&args, &p), json!({}));
}

#[test]
fn skill_and_failure_notes_are_staged_then_drained_once_at_either_boundary() {
    let f = Fixture::new();
    // A skill document may quote command output; read_file content is not a
    // run_command exit status and must not turn a successful skill read into failure.
    f.transcript(&[json!({"step_index":5,"type":"GENERIC","status":"DONE",
        "content":"Command exited with code 1."})]);
    let mut p = f.payload("PostToolUse");
    p["toolCall"] =
        json!({"id":"skill", "name":"read_file", "args":{"Path":"/tmp/skills/testing/SKILL.md"}});
    p["stepIdx"] = json!(5);
    assert_eq!(f.run(&["recall"], &p), json!({}));
    assert_eq!(f.run(&["recall"], &p), json!({}));
    let mut drain = f.payload("PostInvocation");
    assert!(text(&f.run(&["remind"], &drain)).contains("testing skill"));
    assert_eq!(f.run(&["remind"], &drain), json!({}));
    p["toolCall"] = json!({"id":"error", "name":"run_command", "args":{"CommandLine":"build-app"}});
    p["error"] = json!("error: cannot establish app-server socket mount isolation");
    p["stepIdx"] = json!(6);
    assert_eq!(f.run(&["recall"], &p), json!({}));
    drain["hookEventName"] = json!("PreInvocation");
    drain["invocationNum"] = json!(2);
    assert!(text(&f.run(&["remind"], &drain)).contains("socket.md"));
    assert_eq!(f.run(&["remind"], &drain), json!({}));
    let db = rusqlite::Connection::open(f.0.join("wiki/usage.sqlite")).unwrap();
    let count: i64 = db
        .query_row(
            "select count(*) from injections where source in ('recall-skill','recall-failure')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    // Failed reads and arbitrary SKILL.md files do not activate skill-tag recall.
    p["conversationId"] = json!("unrelated");
    p["toolCall"] =
        json!({"id":"bad-skill", "name":"read_file", "args":{"Path":"/tmp/testing/SKILL.md"}});
    p["error"] = json!("");
    assert_eq!(f.run(&["recall"], &p), json!({}));
    drain["conversationId"] = json!("unrelated");
    assert_eq!(f.run(&["remind"], &drain), json!({}));
}

#[test]
fn transcript_failure_and_successful_event_use_only_exact_completed_step() {
    let f = Fixture::new();
    f.transcript(&[
        json!({"step_index":8,"type":"GENERIC","status":"DONE","content":"error: cannot establish app-server socket mount isolation\nCommand exited with code 1."}),
        json!({"step_index":9,"type":"GENERIC","status":"DONE","content":"[main abc1234] fix\nCommand exited with code 0."}),
    ]);
    let mut p = f.payload("PostToolUse");
    p["toolCall"] =
        json!({"id":"failed-build", "name":"run_command", "args":{"CommandLine":"cargo test"}});
    p["stepIdx"] = json!(8);
    assert_eq!(f.run(&["recall"], &p), json!({}));
    let drain = f.payload("PostInvocation");
    assert!(text(&f.run(&["remind"], &drain)).contains("Bash failure"));
    p["toolCall"] =
        json!({"id":"commit", "name":"run_command", "args":{"CommandLine":"git commit -m fix"}});
    // Missing result/status must not claim a successful commit.
    p["stepIdx"] = json!(10);
    assert_eq!(f.run(&["remind", "--event"], &p), json!({}));
    assert_eq!(f.run(&["remind"], &drain), json!({}));
    p["stepIdx"] = json!(9);
    assert_eq!(f.run(&["remind", "--event"], &p), json!({}));
    assert!(text(&f.run(&["remind"], &drain)).contains("committed changes"));
}

#[test]
fn event_failure_replays_do_not_inflate_debugging_streak_and_sync_resets_prompts() {
    let f = Fixture::new();
    let mut p = f.payload("PostToolUse");
    p["toolCall"] =
        json!({"id":"failed", "name":"run_command", "args":{"CommandLine":"cargo test"}});
    p["error"] = json!("exit status 1");
    for _ in 0..3 {
        assert_eq!(f.run(&["remind", "--event"], &p), json!({}));
    }
    let db = rusqlite::Connection::open(f.0.join("wiki/index.sqlite")).unwrap();
    assert_eq!(
        db.query_row("select failures from remind_event_state", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let prompt = f.payload("PreInvocation");
    assert_eq!(f.run(&["remind", "--interval", "2"], &prompt), json!({}));
    f.transcript(&[json!({"type":"PLANNER_RESPONSE","tool_calls":[{"name":"run_command","args":{"CommandLine":"agent-wiki sync"}}]})]);
    let mut prompt = prompt;
    prompt["initialNumSteps"] = json!(10);
    assert_eq!(f.run(&["remind", "--interval", "2"], &prompt), json!({}));
}

#[test]
fn every_noop_and_error_path_is_strict_json_and_pretool_allows() {
    let f = Fixture::new();
    for args in [
        vec!["context"],
        vec!["remind"],
        vec!["remind", "--event"],
        vec!["recall"],
        vec!["record"],
    ] {
        assert_eq!(f.run_raw(&args, "not json", true), json!({}));
        assert_eq!(f.run_raw(&args, "{}", true), json!({}));
    }
    assert_eq!(
        f.run_raw(&["recall", "--unknown-option"], "{}", true),
        json!({})
    );
    assert_eq!(
        f.run(&["recall"], &f.payload("PreToolUse")),
        json!({"decision":"allow"})
    );
    // Failure paths keep PreToolUse's required decision whenever the event is
    // readable: bad flags, and JSON whose other fields do not fit the payload.
    let pretool = f.payload("PreToolUse").to_string();
    assert_eq!(
        f.run_raw(&["recall", "--unknown-option"], &pretool, true),
        json!({"decision":"allow"})
    );
    let mut partial = f.payload("PreToolUse");
    partial["invocationNum"] = json!("not a number");
    for args in [vec!["context"], vec!["remind"], vec!["recall"]] {
        assert_eq!(
            f.run_raw(&args, &partial.to_string(), true),
            json!({"decision":"allow"})
        );
    }
    let mut lock = f.lock();
    let _guard = lock.write().unwrap();
    let start = Instant::now();
    assert_eq!(
        f.run_env(
            &["context"],
            &f.payload("SessionStart").to_string(),
            false,
            &[("AGENT_WIKI_HOOK_LOCK_MS", "100")]
        ),
        json!({})
    );
    assert!(start.elapsed() < Duration::from_secs(2));
    // The wait is capped below Antigravity's 5 s hook timeout.
    let start = Instant::now();
    assert_eq!(
        f.run_env(
            &["context"],
            &f.payload("SessionStart").to_string(),
            false,
            &[("AGENT_WIKI_HOOK_LOCK_MS", "60000")]
        ),
        json!({})
    );
    assert!(start.elapsed() < Duration::from_millis(4500));
}

#[test]
fn hooks_wait_for_a_briefly_held_wiki_lock() {
    let f = Fixture::new();
    f.transcript(&[json!({"step_index":5,"type":"GENERIC","status":"DONE","content":"skill"})]);
    let mut p = f.payload("PostToolUse");
    p["toolCall"] =
        json!({"id":"skill", "name":"read_file", "args":{"Path":"/tmp/skills/testing/SKILL.md"}});
    p["stepIdx"] = json!(5);
    let env = [("AGENT_WIKI_HOOK_LOCK_MS", "2000")];
    let hold = |f: &Fixture| {
        let (tx, rx) = std::sync::mpsc::channel();
        let path = f.0.clone();
        let handle = std::thread::spawn(move || {
            let mut lock = wiki_lock(&path);
            let guard = lock.write().unwrap();
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(400));
            drop(guard);
        });
        rx.recv().unwrap();
        handle
    };
    for (args, payload) in [
        (vec!["context"], f.payload("SessionStart")),
        (vec!["recall"], p.clone()),
    ] {
        let held = hold(&f);
        let start = Instant::now();
        let result = f.run_env(&args, &payload.to_string(), false, &env);
        assert!(
            start.elapsed() >= Duration::from_millis(300),
            "{args:?} did not wait"
        );
        if args == ["context"] {
            assert!(text(&result).contains("Current project: demo"));
        } else {
            assert_eq!(result, json!({}));
        }
        held.join().unwrap();
    }
    assert_eq!(f.pending(), 1);
    let held = hold(&f);
    let result = f.run_env(
        &["remind"],
        &f.payload("PostInvocation").to_string(),
        false,
        &env,
    );
    held.join().unwrap();
    assert!(text(&result).contains("testing skill"));
    assert_eq!(f.pending(), 0);
}

#[test]
fn failing_prompt_reminder_still_delivers_staged_notes() {
    let f = Fixture::new();
    f.transcript(&[json!({"step_index":5,"type":"GENERIC","status":"DONE","content":"skill"})]);
    let mut p = f.payload("PostToolUse");
    p["toolCall"] =
        json!({"id":"skill", "name":"read_file", "args":{"Path":"/tmp/skills/testing/SKILL.md"}});
    p["stepIdx"] = json!(5);
    assert_eq!(f.run(&["recall"], &p), json!({}));
    assert_eq!(f.pending(), 1);
    // A new prompt whose transcript is unreadable makes remind::messages fail.
    let mut prompt = f.payload("PreInvocation");
    prompt["transcriptPath"] = json!(f.0.join("wiki"));
    prompt["initialNumSteps"] = json!(7);
    let result = f.run_raw(&["remind"], &prompt.to_string(), true);
    assert!(text(&result).contains("testing skill"), "{result}");
    assert_eq!(f.pending(), 0);
    let db = rusqlite::Connection::open(f.0.join("wiki/usage.sqlite")).unwrap();
    let logged: i64 = db
        .query_row(
            "select count(*) from injections where source='recall-skill'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(logged, 1);
    prompt["invocationNum"] = json!(2);
    assert_eq!(f.run_raw(&["remind"], &prompt.to_string(), true), json!({}));
}

fn long_output(code: i64, extra: &[&str]) -> String {
    let mut lines: Vec<String> = (0..30)
        .map(|i| format!("   Compiling crate-{i} v0.1.0"))
        .collect();
    lines.insert(
        25,
        "error: cannot establish app-server socket mount isolation".into(),
    );
    lines.extend(extra.iter().map(|s| s.to_string()));
    format!(
        "Created At: 2026-11-26T22:19:04+09:00\nCompleted At: 2026-11-26T22:19:15+09:00\n\
         Command exited with code {code}.\n\nCommand output:\n{}",
        lines.join("\n")
    )
}

#[test]
fn exit_header_is_read_before_long_command_output() {
    let f = Fixture::new();
    f.transcript(&[
        json!({"step_index":8,"type":"GENERIC","status":"DONE","created_at":"2026-11-26T10:00:02Z","content":long_output(1, &[])}),
        json!({"step_index":9,"type":"GENERIC","status":"DONE","created_at":"2026-11-26T10:00:03Z","content":long_output(0, &[])}),
        // Exit 0 header; a quoted exit line in the output body is not a failure.
        json!({"step_index":10,"type":"GENERIC","status":"DONE","created_at":"2026-11-26T10:00:04Z","content":long_output(0, &["Command exited with code 1."])}),
    ]);
    let mut drain = f.payload("PostInvocation");
    let state = |f: &Fixture| -> i64 {
        rusqlite::Connection::open(f.0.join("wiki/index.sqlite"))
            .unwrap()
            .query_row(
                "select coalesce(sum(failures),0) from remind_event_state",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let mut p = f.payload("PostToolUse");
    p["toolCall"] =
        json!({"id":"build", "name":"run_command", "args":{"CommandLine":"cargo build"}});
    p["stepIdx"] = json!(8);
    assert_eq!(f.run(&["recall"], &p), json!({}));
    assert_eq!(f.run(&["remind", "--event"], &p), json!({}));
    let note = f.run(&["remind"], &drain);
    assert!(text(&note).contains("Bash failure"), "{note}");
    assert!(!text(&note).contains("exit status unavailable"), "{note}");
    assert_eq!(state(&f), 1);
    // Exit 0 with long output and a quoted exit line: successful commits, no failures.
    // Each in its own conversation: the commit event fires once per window.
    for (conversation, step) in [("conversation-one", 9), ("trap", 10)] {
        p["conversationId"] = json!(conversation);
        drain["conversationId"] = json!(conversation);
        p["toolCall"] = json!({"id":"commit", "name":"run_command", "args":{"CommandLine":"git commit -m fix"}});
        p["stepIdx"] = json!(step);
        assert_eq!(f.run(&["recall"], &p), json!({}));
        assert_eq!(f.run(&["remind", "--event"], &p), json!({}));
        let note = text(&f.run(&["remind"], &drain)).to_owned();
        assert!(note.contains("committed changes"), "{note}");
        assert!(!note.contains("Bash failure"), "{note}");
        // The commit resolves the streak; the quoted exit line adds no failure.
        assert_eq!(state(&f), 0);
    }
    let mut stop = f.payload("Stop");
    stop["terminationReason"] = json!("NO_TOOL_CALL");
    let mut rows: Vec<Value> = vec![
        json!({"step_index":0,"type":"USER_INPUT","status":"DONE","created_at":"2026-11-26T10:00:00Z","content":"빌드"}),
    ];
    rows.extend(
        fs::read_to_string(f.0.join("transcript.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap()),
    );
    f.transcript(&rows);
    assert_eq!(f.run(&["record", "--wait"], &stop), json!({}));
    let db = rusqlite::Connection::open(f.0.join("state/agent-wiki/records.sqlite")).unwrap();
    let body: String = db
        .query_row(
            "select body from records where session_id='conversation-one'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let r: Value = serde_json::from_str(&body).unwrap();
    let failures = r["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 1, "{r}");
    assert_eq!(
        failures[0]["errors"],
        json!(["error: cannot establish app-server socket mount isolation"])
    );
}

#[test]
fn stop_records_observed_schema_and_detaches_without_holding_stdout() {
    let f = Fixture::new();
    f.transcript(&[
        json!({"step_index":0,"type":"USER_INPUT","status":"DONE","created_at":"2026-11-26T10:00:00Z","content":"고장난 빌드 수정"}),
        json!({"step_index":1,"type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-26T10:00:01Z","tool_calls":[{"name":"run_command","args":{"CommandLine":"false"}}]}),
        json!({"step_index":2,"type":"GENERIC","status":"DONE","created_at":"2026-11-26T10:00:02Z","content":"error: socket unavailable\nCommand exited with code 1."}),
        json!({"future_field":"ignored"}),
    ]);
    let mut p = f.payload("Stop");
    p["terminationReason"] = json!("NO_TOOL_CALL");
    p["executionNum"] = json!(1);
    p["fullyIdle"] = json!(true);
    p["continuationActive"] = json!(true);
    assert_eq!(f.run(&["record", "--wait"], &p), json!({}));
    let db = rusqlite::Connection::open(f.0.join("state/agent-wiki/records.sqlite")).unwrap();
    let body: String = db
        .query_row(
            "select body from records where session_id='conversation-one'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let r: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(r["harness"], "antigravity");
    assert_eq!(r["reason"], "NO_TOOL_CALL");
    assert_eq!(r["cwd"], f.0.join("demo").to_str().unwrap());
    assert_eq!(r["first_prompt"], "고장난 빌드 수정");
    assert_eq!(r["tool_calls"]["run_command"], 1);
    assert_eq!(r["duration_seconds"], 2);
    assert_eq!(r["failures"].as_array().unwrap().len(), 1);
    assert_eq!(r["files_written"], json!([]));
    p["conversationId"] = json!("detached");
    let start = Instant::now();
    assert_eq!(f.run(&["record"], &p), json!({}));
    assert!(start.elapsed() < Duration::from_secs(2));
    loop {
        let count: i64 = db
            .query_row(
                "select count(*) from records where session_id='detached'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if count == 1 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(20));
    }
}

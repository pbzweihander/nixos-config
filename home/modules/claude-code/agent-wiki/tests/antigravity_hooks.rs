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
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-wiki"))
            .args(args)
            .args(["--harness", "antigravity"])
            .current_dir(&self.0)
            .env("AGENT_WIKI_DIR", self.0.join("wiki"))
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("AGENT_WIKI_SOCKET", "")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
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
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(f.0.join("wiki/.lock"))
        .unwrap();
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write().unwrap();
    assert_eq!(f.run(&["context"], &f.payload("SessionStart")), json!({}));
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

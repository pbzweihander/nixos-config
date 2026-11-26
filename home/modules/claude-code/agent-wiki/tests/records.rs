use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn detached_hook_closes_pipes_and_never_opens_the_wiki() {
    for legacy in [false, true] {
        detached_hook(legacy);
    }
}

fn detached_hook(legacy: bool) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("wiki-record-hook-{}-{nonce}", std::process::id());
    let tmp = Temp(std::env::temp_dir().join(name));
    fs::create_dir(&tmp.0).unwrap();
    let state = tmp.0.join(if legacy {
        "state/claude-wiki"
    } else {
        "state/agent-wiki"
    });
    if legacy {
        fs::create_dir_all(&state).unwrap();
    }
    let transcript = tmp.0.join("session.jsonl");
    fs::write(&transcript, include_str!("fixtures/record-codex.jsonl")).unwrap();
    // A regular file makes any attempt to initialize/lock this wiki fail.
    let wiki = tmp.0.join("wiki");
    fs::write(&wiki, "untouched").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-wiki"));
    command
        .arg("record")
        .env("XDG_STATE_HOME", tmp.0.join("state"))
        .env("CLAUDE_WIKI_DIR", &wiki)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = command.spawn().unwrap();
    write!(
        child.stdin.take().unwrap(),
        "{}",
        serde_json::json!({"session_id":"record-codex", "transcript_path":transcript})
    )
    .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(result.stdout.is_empty());
    assert!(start.elapsed() < Duration::from_secs(2));
    let db_path = state.join("records.sqlite");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(db) = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) && db
            .query_row("select count(*) from records", [], |row| {
                row.get::<_, i64>(0)
            })
            .ok()
            == Some(1)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "detached worker did not save a record"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        fs::metadata(&db_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(fs::read_to_string(wiki).unwrap(), "untouched");
}

#[test]
fn backfill_includes_orca_and_exec_and_reports_exclusion_reasons() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("wiki-record-sources-{}-{nonce}", std::process::id());
    let tmp = Temp(std::env::temp_dir().join(name));
    let codex_dir = tmp.0.join("codex/2026/11/20");
    let claude_dir = tmp.0.join("claude/project");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(&claude_dir).unwrap();
    let fixture = include_str!("fixtures/record-orca.jsonl");
    for (name, source, originator) in [
        ("orca", serde_json::json!("vscode"), "orca_desktop"),
        ("exec", serde_json::json!("exec"), "codex_exec"),
        ("unknown", serde_json::json!("exec"), "unknown-runner"),
        (
            "subagent",
            serde_json::json!({"subagent":{"other":"guardian"}}),
            "codex_exec",
        ),
    ] {
        let mut lines: Vec<serde_json::Value> = fixture
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        lines[0]["payload"]["id"] = name.into();
        lines[0]["payload"]["source"] = source;
        lines[0]["payload"]["originator"] = originator.into();
        fs::write(
            codex_dir.join(format!("rollout-{name}.jsonl")),
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    fs::write(claude_dir.join("scripted.jsonl"), "{\"type\":\"user\",\"sessionId\":\"scripted\",\"entrypoint\":\"sdk-cli\",\"message\":{\"content\":\"scripted\"}}\n").unwrap();
    let command = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-wiki"));
        command
            .args(args)
            .env("XDG_STATE_HOME", tmp.0.join("state"))
            .env("CLAUDE_WIKI_SESSIONS_DIRS", tmp.0.join("claude"))
            .env("CLAUDE_WIKI_CODEX_SESSIONS_DIRS", tmp.0.join("codex"))
            .env("CLAUDE_WIKI_DIR", tmp.0.join("wiki"))
            .env_remove("CLAUDE_SESSION_ID")
            .env_remove("CODEX_THREAD_ID");
        command
    };
    let result = command(&["record", "--backfill", "--days", "0"])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.contains("claude=0 codex=3 skipped=2 older=0 failed=0"),
        "{output}"
    );
    assert!(output.contains("\"codex_subagent\":1"), "{output}");
    assert!(output.contains("\"claude_sdk_cli\":1"), "{output}");
    for originator in ["orca_desktop", "codex_exec", "unknown-runner"] {
        assert!(output.contains(originator), "{output}");
    }
    let result = command(&["sessions", "--records", "journal"])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("record orca (codex/orca,"), "{output}");
    assert!(
        output.contains("\"originator\": \"codex_exec\""),
        "{output}"
    );
    assert!(!tmp.0.join("wiki").exists());
}

#[test]
fn curator_exclusion_applies_to_both_formats_and_rebuilds_old_indexes() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp = Temp(std::env::temp_dir().join(format!(
        "wiki-curator-exclusion-{}-{nonce}",
        std::process::id()
    )));
    let codex_dir = tmp.0.join("codex/2026/11/20");
    let claude_dir = tmp.0.join("claude/project");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(&claude_dir).unwrap();
    let command = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_agent-wiki"))
            .args(args)
            .env("HOME", &tmp.0)
            .env("XDG_CACHE_HOME", tmp.0.join("cache"))
            .env("XDG_STATE_HOME", tmp.0.join("state"))
            .env("CLAUDE_WIKI_DIR", tmp.0.join("wiki"))
            .env("CLAUDE_WIKI_SESSIONS_DIRS", tmp.0.join("claude"))
            .env("CLAUDE_WIKI_CODEX_SESSIONS_DIRS", tmp.0.join("codex"))
            .env_remove("CLAUDE_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        String::from_utf8(result.stdout).unwrap()
    };
    for harness in ["claude", "codex"] {
        for name in [
            "marker",
            "marker-in-cache",
            "cwd",
            "default-cache",
            "project",
            "jobs",
            "later-marker",
        ] {
            let cwd = match name {
                "cwd" | "marker-in-cache" => tmp.0.join("cache/wiki-curator/worktree"),
                "default-cache" => tmp.0.join(".cache/wiki-curator/worktree"),
                "jobs" => tmp.0.join(".claude/jobs/12345678/tmp"),
                // A similar name must not trigger a path prefix match.
                _ => tmp.0.join("cache/wiki-curator-project"),
            };
            let id = format!("{harness}-{name}");
            let prompt = if name.starts_with("marker") {
                "[wiki-curator automation]\njournal"
            } else {
                "journal"
            };
            let mut lines = if harness == "codex" {
                vec![
                    serde_json::json!({"type":"session_meta", "payload":{"id":id,"cwd":cwd,"originator":"codex_exec","source":"exec"}}),
                    serde_json::json!({"type":"response_item", "payload":{"type":"message","role":"user","content":[{"type":"input_text","text":prompt}]}}),
                    serde_json::json!({"type":"response_item", "payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"journal completed"}]}}),
                    serde_json::json!({"type":"response_item", "payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"[wiki-curator automation]\nquoted later"}]}}),
                ]
            } else {
                vec![
                    serde_json::json!({"type":"user","sessionId":id,"uuid":format!("{id}-user"),"cwd":cwd,"timestamp":"2026-11-20T11:00:00Z","message":{"content":prompt}}),
                    // Missing cwd must not let assistant rows escape a session-wide exclusion.
                    serde_json::json!({"type":"assistant","sessionId":id,"uuid":format!("{id}-assistant"),"timestamp":"2026-11-20T11:01:00Z","message":{"content":"journal completed"}}),
                    serde_json::json!({"type":"user","sessionId":id,"uuid":format!("{id}-later"),"timestamp":"2026-11-20T11:02:00Z","message":{"content":"[wiki-curator automation]\nquoted later"}}),
                ]
            };
            for line in &mut lines {
                if line.get("timestamp").is_none() {
                    line["timestamp"] = "2026-11-20T11:00:00Z".into();
                }
            }
            let path = if harness == "codex" {
                codex_dir.join(format!("rollout-{name}.jsonl"))
            } else {
                claude_dir.join(format!("{name}.jsonl"))
            };
            fs::write(
                path,
                lines.iter().map(|v| format!("{v}\n")).collect::<String>(),
            )
            .unwrap();
        }
    }
    let output = command(&["record", "--backfill", "--days", "0"]);
    assert!(
        output.contains("claude=3 codex=3 skipped=8 older=0 failed=0"),
        "{output}"
    );
    assert!(output.contains("\"wiki_curator_marker\":4"), "{output}");
    assert!(output.contains("\"wiki_curator_cwd\":4"), "{output}");
    assert!(output.contains("claude_jobs_tmp"), "{output}");
    command(&["sessions", "journal"]);
    let db = rusqlite::Connection::open(tmp.0.join("wiki/sessions.sqlite")).unwrap();
    assert_eq!(
        db.query_row("select count(distinct session_id) from messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        6
    );
    // Simulate the previous format containing a curator row from an unchanged file.
    db.execute_batch("update state set value=3 where key='index_format'; insert into messages(session_id,uuid,text) values('old-curator','old-curator','oldcuratoruniqueword');").unwrap();
    command(&["sessions", "journal"]);
    assert_eq!(
        db.query_row(
            "select count(*) from messages_fts where messages_fts match 'oldcuratoruniqueword'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("select count(distinct session_id) from messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        6
    );
    db.execute_batch("insert into messages(uuid,text) values('retained','retained');")
        .unwrap();
    command(&["sessions", "journal"]);
    assert_eq!(
        db.query_row(
            "select count(*) from messages where uuid='retained'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn decoded_prompts_utc_boundaries_and_denied_writes() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp = Temp(std::env::temp_dir().join(format!(
        "wiki-record-regression-{}-{nonce}",
        std::process::id()
    )));
    fs::create_dir(&tmp.0).unwrap();
    let transcript = tmp.0.join("antigravity.jsonl");
    let run = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_agent-wiki"));
        for (key, _) in std::env::vars()
            .filter(|(k, _)| k.starts_with("AGENT_WIKI_") || k.starts_with("CLAUDE_WIKI_"))
        {
            cmd.env_remove(key);
        }
        let out = cmd
            .args(args)
            .env("HOME", &tmp.0)
            .env("XDG_STATE_HOME", tmp.0.join("state"))
            .env("AGENT_WIKI_DIR", tmp.0.join("wiki"))
            .env("AGENT_WIKI_SOCKET", "")
            .env("AGENT_WIKI_SESSIONS_DIRS", "::")
            .env("AGENT_WIKI_CODEX_SESSIONS_DIRS", "::")
            .env("AGENT_WIKI_ANTIGRAVITY_SESSIONS_DIRS", "::")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let prompt = |text: &str| {
        format!(
            "{}\n",
            serde_json::json!({"step_index":0,"type":"USER_INPUT","status":"DONE","created_at":"2026-11-27T09:00:00+09:00","content":format!("<USER_REQUEST>\n{text}\n</USER_REQUEST>\n<SYSTEM_MESSAGE>injectednoise</SYSTEM_MESSAGE>")})
        )
    };
    fs::write(&transcript, prompt("first\nnewlineunique\ttabunique")).unwrap();
    run(&[
        "record",
        "--wait",
        "--harness",
        "antigravity",
        "--transcript-path",
        transcript.to_str().unwrap(),
        "--session-id",
        "normal",
        "--cwd",
        "/home/u/orca/workspaces/repo/branch/src",
    ]);
    assert!(
        run(&[
            "sessions",
            "--records",
            "newlineunique",
            "--after",
            "2026-11-27",
            "--project",
            "repo"
        ])
        .contains("record normal")
    );
    assert!(
        run(&[
            "sessions",
            "--records",
            "tabunique",
            "--before",
            "2026-11-27"
        ])
        .contains("no session records match")
    );
    assert!(run(&["sessions", "--records", "injectednoise"]).contains("no session records match"));
    let curator = tmp.0.join("curator.jsonl");
    fs::write(&curator, prompt("[wiki-curator automation]\ncuratorunique")).unwrap();
    run(&[
        "record",
        "--wait",
        "--harness",
        "antigravity",
        "--transcript-path",
        curator.to_str().unwrap(),
        "--session-id",
        "curator",
        "--cwd",
        "/custom/worktree",
    ]);
    let db = rusqlite::Connection::open(tmp.0.join("state/agent-wiki/records.sqlite")).unwrap();
    assert_eq!(
        db.query_row("select count(*) from records", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    for (i, response) in [
        serde_json::json!("Approval denied"),
        serde_json::json!("Output:\nExit code: 0\nOutput:\nspoof"),
        serde_json::json!({"exit_code":0}),
    ]
    .into_iter()
    .enumerate()
    {
        let rows = [
            serde_json::json!({"type":"session_meta","payload":{"id":format!("cx-{i}"),"cwd":"/repo"}}),
            serde_json::json!({"type":"response_item","timestamp":"2026-11-27T00:00:00Z","payload":{"type":"function_call","name":"exec_command","call_id":"call","arguments":"{\"cmd\":\"echo hi > output.txt\"}"}}),
            serde_json::json!({"type":"response_item","timestamp":"2026-11-27T00:00:01Z","payload":{"type":"function_call_output","call_id":"call","output":response}}),
        ];
        fs::write(
            &transcript,
            rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        run(&[
            "record",
            "--wait",
            "--transcript-path",
            transcript.to_str().unwrap(),
        ]);
        let body: String = db
            .query_row(
                "select body from records where session_id=?",
                [format!("cx-{i}")],
                |r| r.get(0),
            )
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            body["files_written"].as_array().unwrap().len(),
            usize::from(i == 2)
        );
    }
}

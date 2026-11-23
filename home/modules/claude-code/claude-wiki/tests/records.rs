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
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("wiki-record-hook-{}-{nonce}", std::process::id());
    let tmp = Temp(std::env::temp_dir().join(name));
    fs::create_dir(&tmp.0).unwrap();
    let transcript = tmp.0.join("session.jsonl");
    fs::write(&transcript, include_str!("fixtures/record-codex.jsonl")).unwrap();
    // A regular file makes any attempt to initialize/lock this wiki fail.
    let wiki = tmp.0.join("wiki");
    fs::write(&wiki, "untouched").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_claude-wiki"));
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
    let db_path = tmp.0.join("state/claude-wiki/records.sqlite");
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_claude-wiki"));
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

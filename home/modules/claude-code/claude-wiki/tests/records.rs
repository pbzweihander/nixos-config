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

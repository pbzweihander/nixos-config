//! `agent-wiki sessions` full-text search over Antigravity transcripts, end to end.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp() -> Temp {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "agent-wiki-antigravity-sessions-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    Temp(path)
}

fn step(index: u64, kind: &str, extra: Value) -> String {
    let mut value = json!({
        "step_index": index, "source": "MODEL", "type": kind, "status": "DONE",
        "created_at": format!("2026-11-26T11:{index:02}:00Z"),
    });
    for (key, field) in extra.as_object().unwrap() {
        value[key] = field.clone();
    }
    format!("{value}\n")
}

/// Writes a conversation under `<app>/brain` and records its workspace in the app's
/// metadata cache, as Antigravity does.
fn conversation(app: &Path, id: &str, cwd: &str, word: &str) {
    let logs = app.join("brain").join(id).join(".system_generated/logs");
    fs::create_dir_all(&logs).unwrap();
    let prompt = format!(
        "<USER_REQUEST>\nplease fix the {word} lattice\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\nmetadatanoise\n</ADDITIONAL_METADATA>\n<SYSTEM_MESSAGE>\nhooknoise\n</SYSTEM_MESSAGE>"
    );
    let text = [
        step(0, "USER_INPUT", json!({"content": prompt})),
        step(
            1,
            "PLANNER_RESPONSE",
            json!({"content": format!("the {word} lattice answer"), "thinking": "thinkingonly lattice",
                   "tool_calls": [{"name": "run_command", "args": {"CommandLine": "toolcallonly"}}]}),
        ),
        step(2, "GENERIC", json!({"content": "genericonly lattice"})),
        step(3, "SYSTEM_MESSAGE", json!({"content": "systemrowonly lattice"})),
    ]
    .concat();
    fs::write(logs.join("transcript.jsonl"), text).unwrap();
    let cache = app.join("cache");
    fs::create_dir_all(&cache).unwrap();
    let path = cache.join("conversation_metadata.json");
    let mut value: Value = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({"conversations": {}}));
    value["conversations"][id] =
        json!({"summary": {"ID": id, "WorkspaceURIs": [format!("file://{cwd}")]}});
    fs::write(path, value.to_string()).unwrap();
}

struct Env {
    tmp: Temp,
}

impl Env {
    fn new() -> Self {
        let tmp = temp();
        let home = &tmp.0;
        let claude = home.join("claude/project");
        fs::create_dir_all(&claude).unwrap();
        fs::write(
            claude.join("session.jsonl"),
            format!(
                "{}\n",
                json!({"type": "user", "sessionId": "claude-session", "uuid": "c1",
                       "timestamp": "2026-11-26T10:00:00Z", "cwd": "/repos/demo",
                       "message": {"content": "claudeword lattice"}})
            ),
        )
        .unwrap();
        let gemini = home.join(".gemini");
        let bots = home.join(".orca/bots/bot-2");
        conversation(
            &gemini.join("antigravity-cli"),
            "conv-orca",
            bots.to_str().unwrap(),
            "orcaword",
        );
        conversation(
            &gemini.join("antigravity-ide"),
            "conv-plain",
            "/repos/plainrepo",
            "plainword",
        );
        Self { tmp }
    }

    fn run(&self, args: &[&str], envs: &[(&str, &str)]) -> String {
        let home = &self.tmp.0;
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-wiki"));
        command
            .arg("sessions")
            .args(args)
            .env("HOME", home)
            .env("AGENT_WIKI_DIR", home.join("wiki"))
            .env("AGENT_WIKI_SOCKET", home.join("no-daemon.sock"))
            .env("AGENT_WIKI_SESSIONS_DIRS", home.join("claude"))
            .env("AGENT_WIKI_CODEX_SESSIONS_DIRS", home.join("codex"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        for key in [
            "AGENT_WIKI_ANTIGRAVITY_SESSIONS_DIRS",
            "CLAUDE_WIKI_ANTIGRAVITY_SESSIONS_DIRS",
            "CLAUDE_WIKI_DIR",
            "CLAUDE_SESSION_ID",
            "CODEX_THREAD_ID",
            "ANTIGRAVITY_CONVERSATION_ID",
        ] {
            command.env_remove(key);
        }
        command.envs(envs.iter().copied());
        let out = command.output().unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    }
}

fn sessions(out: &str) -> Vec<&str> {
    out.lines()
        .filter_map(|line| line.strip_prefix("session "))
        .collect()
}

#[test]
fn default_dirs_are_searched_with_harness_labels_and_filters() {
    let env = Env::new();
    let out = env.run(&["lattice"], &[]);
    let found = sessions(&out);
    assert_eq!(found.len(), 3, "{out}");
    assert!(
        found
            .iter()
            .any(|s| s.starts_with("conv-orca  (antigravity/orca, bot-2, 2026-11-26"))
    );
    assert!(
        found
            .iter()
            .any(|s| s.starts_with("conv-plain  (antigravity, plainrepo, 2026-11-26"))
    );
    assert!(
        found
            .iter()
            .any(|s| s.starts_with("claude-session  (claude,"))
    );
    assert!(out.contains("\"please fix the orcaword lattice\""), "{out}");
    assert!(out.contains("[assistant 2026-11-26 11:01]"), "{out}");
    for noise in [
        "metadatanoise",
        "hooknoise",
        "thinkingonly",
        "toolcallonly",
        "genericonly",
        "systemrowonly",
    ] {
        assert!(!out.contains(noise), "{noise}: {out}");
        assert!(
            env.run(&[noise], &[]).starts_with("no sessions match"),
            "{noise}"
        );
    }

    // codex/orca matches --harness codex; antigravity/orca matches --harness antigravity.
    let out = env.run(&["lattice", "--harness", "antigravity"], &[]);
    let mut found = sessions(&out);
    found.sort();
    assert_eq!(found.len(), 2, "{out}");
    assert!(found[0].starts_with("conv-orca") && found[1].starts_with("conv-plain"));
    let out = env.run(&["lattice", "--harness", "claude"], &[]);
    assert_eq!(sessions(&out).len(), 1, "{out}");
    assert!(sessions(&out)[0].starts_with("claude-session"));
    assert!(
        env.run(&["orcaword", "--harness", "codex"], &[])
            .starts_with("no sessions match")
    );
    let out = env.run(&["lattice", "--project", "plainrepo"], &[]);
    assert_eq!(sessions(&out).len(), 1, "{out}");
    let out = env.run(
        &["lattice", "--harness", "antigravity", "--role", "user"],
        &[],
    );
    assert!(
        out.contains("[user ") && !out.contains("[assistant "),
        "{out}"
    );
}

#[test]
fn current_conversation_is_excluded() {
    let env = Env::new();
    let out = env.run(&["lattice", "--exclude-session", "conv-orca"], &[]);
    assert!(!out.contains("session conv-orca"), "{out}");
    assert_eq!(sessions(&out).len(), 2, "{out}");
    let out = env.run(
        &["lattice"],
        &[("ANTIGRAVITY_CONVERSATION_ID", "conv-plain")],
    );
    assert!(!out.contains("session conv-plain"), "{out}");
    assert_eq!(sessions(&out).len(), 2, "{out}");
    // An empty id excludes nothing.
    let out = env.run(&["lattice"], &[("ANTIGRAVITY_CONVERSATION_ID", "")]);
    assert_eq!(sessions(&out).len(), 3, "{out}");
}

#[test]
fn source_dirs_override_and_older_index_rebuilds() {
    let env = Env::new();
    let home = &env.tmp.0;
    // An index from before Antigravity support: format 4 with a stale row. Only the
    // version changes, so the triggers and FTS table match a real older index.
    env.run(&["lattice"], &[]);
    {
        let db = rusqlite::Connection::open(home.join("wiki-state/sessions.sqlite")).unwrap();
        db.execute_batch(
            "update state set value = 4 where key = 'index_format';
             insert into messages(session_id, uuid, ts, role, project, branch, text, harness)
                 values ('stale', 'stale', '2026-11-01T00:00:00.000Z', 'user', '', '', 'staleword lattice', 'claude');",
        )
        .unwrap();
    }
    let custom = home.join("custom-app");
    conversation(&custom, "conv-custom", "/repos/customrepo", "customword");
    let only_custom = custom.join("brain");
    let list =
        std::env::join_paths([home.join("missing"), only_custom.clone(), only_custom]).unwrap();
    let list = list.to_str().unwrap();
    let out = env.run(
        &["lattice", "--harness", "antigravity"],
        &[("AGENT_WIKI_ANTIGRAVITY_SESSIONS_DIRS", list)],
    );
    let found = sessions(&out);
    assert_eq!(found.len(), 1, "{out}");
    assert!(found[0].starts_with("conv-custom  (antigravity, customrepo,"));
    assert!(
        env.run(&["staleword"], &[])
            .starts_with("no sessions match"),
        "format bump must drop rows from the older index"
    );
    // The rebuild happens once: a row added after it survives the next run.
    {
        let db = rusqlite::Connection::open(home.join("wiki-state/sessions.sqlite")).unwrap();
        db.execute_batch(
            "insert into messages(session_id, uuid, ts, role, project, branch, text, harness)
                 values ('kept', 'kept', '2026-11-01T00:00:00.000Z', 'user', '', '', 'keptword', 'claude');",
        )
        .unwrap();
    }
    assert!(env.run(&["keptword"], &[]).contains("session kept"));
    let db = rusqlite::Connection::open(home.join("wiki-state/sessions.sqlite")).unwrap();
    let format: i64 = db
        .query_row(
            "select value from state where key='index_format'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(format > 4);
    // The default dirs replace the override once it is unset; transcripts outside
    // the listed directories leave the index.
    let out = env.run(&["lattice", "--harness", "antigravity"], &[]);
    assert_eq!(sessions(&out).len(), 2, "{out}");
    assert!(!out.contains("conv-custom"), "{out}");
}

fn raw_conversation(app: &Path, id: &str, prompt: &str) -> PathBuf {
    let logs = app.join("brain").join(id).join(".system_generated/logs");
    fs::create_dir_all(&logs).unwrap();
    let path = logs.join("transcript.jsonl");
    fs::write(&path, step(0, "USER_INPUT", json!({"content": prompt}))).unwrap();
    path
}

#[test]
fn literal_tags_inside_the_request_stay_searchable() {
    let env = Env::new();
    let app = env.tmp.0.join(".gemini/antigravity-cli");
    let tail = "\r\n<ADDITIONAL_METADATA>\r\nmetadatanoise\r\n</ADDITIONAL_METADATA>\n<SYSTEM_MESSAGE>\nhooknoise\n</SYSTEM_MESSAGE>";
    raw_conversation(
        &app,
        "nested",
        &format!(
            "<USER_REQUEST>beforeword <USER_REQUEST>literalword</USER_REQUEST> afterword</USER_REQUEST>{tail}"
        ),
    );
    raw_conversation(
        &app,
        "xml",
        &format!(
            "<USER_REQUEST>Explain this XML: <SYSTEM_MESSAGE>payloadword</SYSTEM_MESSAGE> tailword\r\n\"The following is a <SYSTEM_MESSAGE> not actually sent by the user.\" 한글끝</USER_REQUEST>{tail}"
        ),
    );
    for (word, session) in [
        ("beforeword", "nested"),
        ("literalword", "nested"),
        ("afterword", "nested"),
        ("payloadword", "xml"),
        ("tailword", "xml"),
        ("한글끝", "xml"),
    ] {
        let out = env.run(&[word, "--harness", "antigravity"], &[]);
        assert!(
            out.contains(&format!("session {session} ")),
            "{word}: {out}"
        );
    }
    let out = env.run(&["afterword"], &[]);
    assert!(
        out.contains("\"beforeword <USER_REQUEST>literalword</USER_REQUEST> afterword\""),
        "{out}"
    );
    for noise in ["metadatanoise", "hooknoise"] {
        assert!(
            env.run(&[noise], &[]).starts_with("no sessions match"),
            "{noise}"
        );
    }
}

#[test]
fn late_metadata_relabels_and_excludes_curator_runs() {
    let env = Env::new();
    let app = env.tmp.0.join(".gemini/antigravity-cli");
    raw_conversation(&app, "late", "<USER_REQUEST>latecuratorword</USER_REQUEST>");
    let out = env.run(&["latecuratorword"], &[]);
    assert!(out.contains("session late  (antigravity, ,"), "{out}");
    let metadata = |uris: Value| {
        let path = app.join("cache/conversation_metadata.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["conversations"]["late"] = json!({"summary": {"WorkspaceURIs": uris}});
        fs::write(path, value.to_string()).unwrap();
    };
    let bot = env.tmp.0.join(".orca/bots/bot-9");
    metadata(json!([
        "file://remotehost/elsewhere",
        format!("file://localhost{}", bot.display())
    ]));
    let out = env.run(&["latecuratorword"], &[]);
    assert!(
        out.contains("session late  (antigravity/orca, bot-9,"),
        "{out}"
    );
    let curator = env.tmp.0.join("cache/wiki-curator/worktree");
    metadata(json!([format!("file://{}", curator.display())]));
    assert!(
        env.run(&["latecuratorword"], &[])
            .starts_with("no sessions match")
    );
}

#[test]
fn inaccessible_conversations_do_not_break_search() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let app = env.tmp.0.join(".gemini/antigravity-cli");
    let path = raw_conversation(
        &app,
        "locked",
        "<USER_REQUEST>lockedword lattice</USER_REQUEST>",
    );
    let logs = path.parent().unwrap().to_path_buf();
    fs::set_permissions(&logs, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::metadata(&path).is_ok();
    let out = (!readable).then(|| env.run(&["lattice"], &[]));
    fs::set_permissions(&logs, fs::Permissions::from_mode(0o755)).unwrap();
    // root ignores permission bits; there is nothing to exercise then.
    if let Some(out) = out {
        assert_eq!(sessions(&out).len(), 3, "{out}");
        assert!(!out.contains("lockedword"), "{out}");
    }
}

#[test]
fn legacy_dirs_setting_is_a_fallback() {
    let env = Env::new();
    let home = &env.tmp.0;
    conversation(
        &home.join("legacy-app"),
        "conv-legacy",
        "/repos/legacyrepo",
        "legacyword",
    );
    let legacy = home.join("legacy-app/brain");
    let legacy = legacy.to_str().unwrap();
    let out = env.run(
        &["lattice", "--harness", "antigravity"],
        &[("CLAUDE_WIKI_ANTIGRAVITY_SESSIONS_DIRS", legacy)],
    );
    assert_eq!(sessions(&out).len(), 1, "{out}");
    assert!(out.contains("session conv-legacy "), "{out}");
    // The new name wins, and an empty new value means the defaults.
    let out = env.run(
        &["lattice", "--harness", "antigravity"],
        &[
            ("CLAUDE_WIKI_ANTIGRAVITY_SESSIONS_DIRS", legacy),
            ("AGENT_WIKI_ANTIGRAVITY_SESSIONS_DIRS", ""),
        ],
    );
    assert_eq!(sessions(&out).len(), 2, "{out}");
    assert!(!out.contains("conv-legacy"), "{out}");
}

#[test]
fn cross_harness_usage_scans_and_orders_subsecond_reads() {
    let env = Env::new();
    let home = &env.tmp.0;
    env.run(&["lattice"], &[]);
    let page = home.join("wiki/pages/knowledge/usage.md");
    fs::write(
        &page,
        "---\ntitle: 사용량 검증\ncreated: 2026-11-27\n---\n검증 페이지\n",
    )
    .unwrap();
    let ag = home
        .join(".gemini/antigravity-cli/brain/conv-orca/.system_generated/logs/transcript.jsonl");
    fs::write(&ag, format!("{}\n", json!({"step_index":0,"type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-27T00:00:00.100Z","tool_calls":[{"name":"read_file","args":{"Path":page}}]}))).unwrap();
    fs::create_dir_all(home.join("codex")).unwrap();
    fs::write(home.join("codex/rollout-usage.jsonl"), format!("{}\n{}\n", json!({"type":"session_meta","payload":{"id":"cx","cwd":"/repo"}}), json!({"type":"response_item","timestamp":"2026-11-27T00:00:00.100Z","payload":{"type":"function_call","name":"exec_command","call_id":"r","arguments":json!({"cmd":format!("cat {}", page.display())}).to_string()}}))).unwrap();
    let stats = || {
        let out = Command::new(env!("CARGO_BIN_EXE_agent-wiki"))
            .args(["stats", "--json"])
            .env("HOME", home)
            .env("AGENT_WIKI_DIR", home.join("wiki"))
            .env("AGENT_WIKI_SESSIONS_DIRS", "::")
            .env("AGENT_WIKI_CODEX_SESSIONS_DIRS", home.join("codex"))
            .env(
                "AGENT_WIKI_ANTIGRAVITY_SESSIONS_DIRS",
                home.join(".gemini/antigravity-cli/brain"),
            )
            .env("AGENT_WIKI_SOCKET", "")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    stats();
    let db = rusqlite::Connection::open(home.join("wiki-state/usage.sqlite")).unwrap();
    for session in ["cx", "conv-orca"] {
        db.execute(
            "insert into injections values (?, 'pages/knowledge/usage.md', 'context', 1795737600)",
            [session],
        )
        .unwrap();
    }
    assert_eq!(stats()["sources"]["context"]["read_later"], 2);
    // A precise injection after the read in the same second must remain unread.
    db.execute_batch("insert into injection_clock select rowid, 1795737600.200 from injections;")
        .unwrap();
    assert_eq!(stats()["sources"]["context"]["read_later"], 0);
    // Reparse actual whole-second sources with a precise .200 injection.
    for source in [&ag, &home.join("codex/rollout-usage.jsonl")] {
        let text = fs::read_to_string(source)
            .unwrap()
            .replace("00:00:00.100Z", "00:00:00Z");
        fs::write(source, text).unwrap();
    }
    db.execute_batch("delete from tools; delete from accesses; delete from transcripts;")
        .unwrap();
    let report = stats();
    assert_eq!(report["sources"]["context"]["read_later"], 2);
    assert!(
        report["injected_without_read"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // An existing v4 DB must also work without any remaining source files.
    fs::remove_file(&ag).unwrap();
    fs::remove_file(home.join("codex/rollout-usage.jsonl")).unwrap();
    db.execute_batch(
        "delete from injection_clock; update state set value=4 where key='scan_format';",
    )
    .unwrap();
    let report = stats();
    assert_eq!(report["sources"]["context"]["read_later"], 2);
    assert!(
        report["injected_without_read"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

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
        let db = rusqlite::Connection::open(home.join("wiki/sessions.sqlite")).unwrap();
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
        let db = rusqlite::Connection::open(home.join("wiki/sessions.sqlite")).unwrap();
        db.execute_batch(
            "insert into messages(session_id, uuid, ts, role, project, branch, text, harness)
                 values ('kept', 'kept', '2026-11-01T00:00:00.000Z', 'user', '', '', 'keptword', 'claude');",
        )
        .unwrap();
    }
    assert!(env.run(&["keptword"], &[]).contains("session kept"));
    let db = rusqlite::Connection::open(home.join("wiki/sessions.sqlite")).unwrap();
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

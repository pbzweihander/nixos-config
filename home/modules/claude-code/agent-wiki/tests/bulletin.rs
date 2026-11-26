use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("wiki-bulletin-{}", std::process::id()));
        fs::create_dir_all(path.join("wiki/pages/knowledge")).unwrap();
        fs::create_dir_all(path.join(".orca/bots/bot-3/work")).unwrap();
        fs::create_dir_all(path.join("project")).unwrap();
        fs::write(
            path.join("wiki/pages/knowledge/notice.md"),
            "---\ntitle: Team SOP notice\ncreated: 2026-11-26\ntags: [orca, bulletin]\n---\nUse worktrees.\n",
        )
        .unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str], input: Option<Value>) -> String {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-wiki"))
            .args(args)
            .current_dir(self.0.join("project"))
            .env("AGENT_WIKI_DIR", self.0.join("wiki"))
            .env("AGENT_WIKI_SOCKET", "")
            .env_remove("ORCA_BOT_HOME")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if let Some(input) = input {
            stdin.write_all(input.to_string().as_bytes()).unwrap();
        }
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn session_start_and_prompts_announce_each_change_once_per_consumer() {
    let fixture = Fixture::new();
    let bot = fixture.0.join(".orca/bots/bot-3/work");
    let prompt = |session: &str, cwd: &PathBuf| {
        Some(json!({"session_id": session, "cwd": cwd,
            "hook_event_name": "UserPromptSubmit", "prompt": "<USER_REQUEST>hi</USER_REQUEST>"}))
    };
    // SessionStart announces and marks seen, so the first prompt stays quiet.
    let start = fixture.run(
        &["context"],
        Some(json!({"session_id": "b1", "cwd": bot, "hook_event_name": "SessionStart"})),
    );
    assert!(start.contains("agent-wiki bulletin: updated notice(s) since your last check: "));
    assert!(start.contains("notice.md (Team SOP notice — updated "));
    let remind = ["remind", "--interval", "100"];
    assert_eq!(fixture.run(&remind, prompt("b1", &bot)), "");
    // Same bot, new session: already seen by the bot.
    assert_eq!(fixture.run(&remind, prompt("b2", &bot)), "");
    // An ordinary session gets it on its `<`-wrapped prompt, once.
    let project = fixture.0.join("project");
    assert!(
        fixture
            .run(&remind, prompt("s1", &project))
            .contains("agent-wiki bulletin:")
    );
    assert_eq!(fixture.run(&remind, prompt("s1", &project)), "");

    let listing: Value = serde_json::from_str(
        &fixture.run(&["bulletin", "--consumer", "session:s9", "--json"], None),
    )
    .unwrap();
    assert_eq!(listing["consumer"], "session:s9");
    assert_eq!(listing["pages"][0]["status"], "new");
    assert_eq!(listing["pages"][0]["path"], "pages/knowledge/notice.md");
    let marked = fixture.run(
        &["bulletin", "--consumer", "session:s9", "--mark-seen"],
        None,
    );
    assert!(marked.contains("marked 1 page(s) seen"), "{marked}");
    assert!(
        fixture
            .run(&["bulletin", "--consumer", "session:s9"], None)
            .contains("seen ")
    );
    let bot_view = fixture.run(&["bulletin", "--cwd", bot.to_str().unwrap()], None);
    assert!(
        bot_view.starts_with("consumer: bot:bot-3\nseen "),
        "{bot_view}"
    );

    // An edit (new mtime) re-announces to the bot.
    let page = fixture.0.join("wiki/pages/knowledge/notice.md");
    fs::File::options()
        .write(true)
        .open(&page)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(5))
        .unwrap();
    assert!(
        fixture
            .run(&remind, prompt("b2", &bot))
            .contains("agent-wiki bulletin:")
    );
    assert_eq!(fixture.run(&remind, prompt("b2", &bot)), "");
}

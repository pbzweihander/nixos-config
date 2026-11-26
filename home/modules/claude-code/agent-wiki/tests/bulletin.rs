use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};

const NOTICE: &str = "---\ntitle: Team SOP notice\ncreated: 2026-11-26\ntags: [orca, bulletin]\n---\nUse worktrees.\n";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "wiki-bulletin-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("wiki/pages/knowledge")).unwrap();
        fs::create_dir_all(path.join(".orca/bots/bot-3/work")).unwrap();
        fs::create_dir_all(path.join(".orca/bots/bot-2/sub")).unwrap();
        fs::create_dir_all(path.join("project")).unwrap();
        let path = path.canonicalize().unwrap();
        let fixture = Self(path);
        fixture.write("notice", NOTICE);
        fixture
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let page = self.0.join(format!("wiki/pages/knowledge/{name}.md"));
        fs::write(&page, text).unwrap();
        page
    }

    fn bot(&self, rel: &str) -> PathBuf {
        self.0.join(".orca/bots").join(rel)
    }

    fn output(&self, args: &[&str], input: Option<String>, bot_home: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-wiki"));
        command
            .args(args)
            .current_dir(self.0.join("project"))
            .env("AGENT_WIKI_DIR", self.0.join("wiki"))
            .env("AGENT_WIKI_SOCKET", "")
            .env_remove("ORCA_BOT_HOME")
            .env_remove("CLAUDE_WIKI_DIR")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(home) = bot_home {
            command.env("ORCA_BOT_HOME", home);
        }
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if let Some(input) = input {
            stdin.write_all(input.as_bytes()).unwrap();
        }
        drop(stdin);
        child.wait_with_output().unwrap()
    }

    fn run_env(&self, args: &[&str], input: Option<Value>, bot_home: Option<&str>) -> String {
        let output = self.output(args, input.map(|v| v.to_string()), bot_home);
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn run(&self, args: &[&str], input: Option<Value>) -> String {
        self.run_env(args, input, None)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set_mtime(page: &Path, secs: u64) {
    fs::File::options()
        .write(true)
        .open(page)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

fn prompt(session: &str, cwd: &Path) -> Option<Value> {
    Some(json!({"session_id": session, "cwd": cwd,
        "hook_event_name": "UserPromptSubmit", "prompt": "<USER_REQUEST>hi</USER_REQUEST>"}))
}

fn start(session: &str, cwd: &Path) -> Option<Value> {
    Some(json!({"session_id": session, "cwd": cwd, "hook_event_name": "SessionStart"}))
}

const REMIND: [&str; 3] = ["remind", "--interval", "100"];
const ANNOUNCE: &str = "agent-wiki bulletin: updated notice(s) since your last check: ";

#[test]
fn session_start_and_prompts_announce_each_change_once_per_consumer() {
    let fixture = Fixture::new();
    let bot = fixture.bot("bot-3/work");
    // SessionStart announces and marks seen, so the first prompt stays quiet.
    let out = fixture.run(&["context"], start("b1", &bot));
    assert!(out.contains(ANNOUNCE));
    assert!(out.contains("notice.md (Team SOP notice — updated "));
    assert_eq!(fixture.run(&REMIND, prompt("b1", &bot)), "");
    // Same bot, new session: already seen by the bot.
    assert_eq!(fixture.run(&REMIND, prompt("b2", &bot)), "");
    // An ordinary session gets it on its `<`-wrapped prompt, once.
    let project = fixture.0.join("project");
    assert!(
        fixture
            .run(&REMIND, prompt("s1", &project))
            .contains(ANNOUNCE)
    );
    assert_eq!(fixture.run(&REMIND, prompt("s1", &project)), "");
    // An edit re-announces to the bot.
    set_mtime(&fixture.0.join("wiki/pages/knowledge/notice.md"), 5);
    assert!(fixture.run(&REMIND, prompt("b2", &bot)).contains(ANNOUNCE));
    assert_eq!(fixture.run(&REMIND, prompt("b2", &bot)), "");
}

#[test]
fn context_picks_up_an_on_disk_edit_without_a_refresh() {
    let fixture = Fixture::new();
    let project = fixture.0.join("project");
    assert!(
        fixture
            .run(&["context"], start("s1", &project))
            .contains(ANNOUNCE)
    );
    assert!(
        !fixture
            .run(&["context"], start("s1", &project))
            .contains(ANNOUNCE)
    );
    fixture.write("notice", &format!("{NOTICE}Edited.\n"));
    set_mtime(&fixture.0.join("wiki/pages/knowledge/notice.md"), 7);
    assert!(
        fixture
            .run(&["context"], start("s1", &project))
            .contains(ANNOUNCE)
    );
}

#[test]
fn bot_resolution_through_hooks_rejects_escapes_and_follows_symlinks() {
    let fixture = Fixture::new();
    let project = fixture.0.join("project");
    symlink(&project, fixture.bot("bot-3/alias")).unwrap();
    symlink(fixture.bot("bot-3/work"), fixture.0.join("link")).unwrap();
    symlink(fixture.bot("bot-2/sub"), fixture.bot("bot-3/cross")).unwrap();
    let probes = std::cell::Cell::new(0);
    let key = |cwd: PathBuf, bot_home: Option<&str>| -> Value {
        // A fresh session per probe: it has seen the notice only if it was the consumer.
        probes.set(probes.get() + 1);
        let session = format!("probe{}", probes.get());
        fixture.run_env(&REMIND, prompt(&session, &cwd), bot_home);
        let consumer = format!("session:{session}");
        let listing: Value = serde_json::from_str(
            &fixture.run(&["bulletin", "--consumer", &consumer, "--json"], None),
        )
        .unwrap();
        listing["pages"][0]["status"].clone()
    };
    // The probe session sees the notice only when the cwd is NOT a bot.
    assert_eq!(key(fixture.bot("bot-3/../../../project"), None), "seen");
    assert_eq!(key(fixture.bot("bot-3/alias"), None), "seen");
    assert_eq!(
        key(fixture.bot("bot-3/missing/../../../project"), None),
        "seen"
    );
    assert_eq!(key(fixture.0.join("link"), None), "new");
    assert_eq!(key(fixture.bot("bot-3/cross"), None), "new");
    assert_eq!(key(fixture.bot("bot-3/missing/../work"), None), "new");
    // ORCA_BOT_HOME wins over the cwd, with the same resolution.
    let alias = fixture.bot("bot-3/alias");
    assert_eq!(
        key(fixture.bot("bot-3/work"), Some(alias.to_str().unwrap())),
        "seen"
    );
    assert_eq!(
        key(
            project.clone(),
            Some(fixture.bot("bot-2").to_str().unwrap())
        ),
        "new"
    );
    // Empty ORCA_BOT_HOME is unset.
    assert_eq!(key(project.clone(), Some("")), "seen");

    // Which bot got it: the CLI with --cwd / ORCA_BOT_HOME reads the same keys.
    let view = |args: &[&str], bot_home: Option<&str>| fixture.run_env(args, None, bot_home);
    assert!(
        view(
            &[
                "bulletin",
                "--cwd",
                fixture.0.join("link").to_str().unwrap()
            ],
            None
        )
        .starts_with("consumer: bot:bot-3\nseen ")
    );
    assert!(
        view(
            &[
                "bulletin",
                "--cwd",
                fixture.bot("bot-3/cross").to_str().unwrap()
            ],
            None
        )
        .starts_with("consumer: bot:bot-2\nseen ")
    );
    assert!(
        view(
            &[
                "bulletin",
                "--cwd",
                fixture.bot("bot-3/work").to_str().unwrap()
            ],
            Some(fixture.bot("bot-2").to_str().unwrap())
        )
        .starts_with("consumer: bot:bot-2\n")
    );
}

#[test]
fn payload_fields_missing_empty_or_not_strings() {
    let fixture = Fixture::new();
    // No cwd: the process cwd (project) is used; session decides.
    let out = fixture.run(&["context"], Some(json!({"session_id": "s1"})));
    assert!(out.contains(ANNOUNCE));
    // Non-string cwd and session: no consumer, nothing announced or marked.
    let out = fixture.run(&["context"], Some(json!({"session_id": 5, "cwd": 7})));
    assert!(!out.contains(ANNOUNCE));
    let out = fixture.run(&["context"], Some(json!({"session_id": "", "cwd": ""})));
    assert!(!out.contains(ANNOUNCE));
    // Non-string cwd with a session falls back to the session.
    let out = fixture.run(
        &["context"],
        Some(json!({"session_id": "s2", "cwd": ["x"]})),
    );
    assert!(out.contains(ANNOUNCE));
    // Terminal-less empty stdin: no payload, no announcement.
    assert!(!fixture.run(&["context"], None).contains(ANNOUNCE));
    assert_eq!(fixture.run(&REMIND, None), "");
    let listing: Value =
        serde_json::from_str(&fixture.run(&["bulletin", "--consumer", "session:", "--json"], None))
            .unwrap();
    assert_eq!(listing["consumer"], "session:");
    assert_eq!(listing["pages"][0]["status"], "new");
}

#[test]
fn many_bulletins_stay_within_the_context_budget_and_arrive_over_turns() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("wiki/pages/followups")).unwrap();
    for i in 0..66 {
        let page = fixture.write(
            &format!("공지-{i:02}-{}", "가".repeat(40)),
            &format!(
                "---\ntitle: \"{i} 한국어 공지 🚀 {}\"\ncreated: 2026-11-26\ntags: [bulletin]\n---\nx\n",
                "긴제목".repeat(60)
            ),
        );
        set_mtime(&page, 10_000 + i);
    }
    let project = fixture.0.join("project");
    let out = fixture.run(&["context"], start("s1", &project));
    let note = out.lines().find(|l| l.starts_with(ANNOUNCE)).unwrap();
    assert!(note.chars().count() <= 1200, "{}", note.chars().count());
    assert!(note.contains(" more (run `agent-wiki bulletin`)"));
    let footer = out.lines().last().unwrap();
    assert!(footer.starts_with("[context "), "{footer}");
    assert!(out.chars().count() <= 6000);
    // Tiny budget: nothing delivered, nothing marked, output still bounded.
    let tiny = fixture.run(&["context", "--budget", "300"], start("s2", &project));
    assert!(!tiny.contains(ANNOUNCE));
    // remind delivers the remainder over later turns, at most 1500 chars each.
    let mut turns = 0;
    loop {
        let out = fixture.run(&REMIND, prompt("s1", &project));
        if out.is_empty() {
            break;
        }
        assert!(out.trim_end().chars().count() <= 1500);
        turns += 1;
        assert!(turns < 20);
    }
    assert!(turns >= 5, "{turns}");
    let listing: Value = serde_json::from_str(
        &fixture.run(&["bulletin", "--consumer", "session:s1", "--json"], None),
    )
    .unwrap();
    let pages = listing["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 67);
    assert!(pages.iter().all(|p| p["status"] == "seen"));
    let listing: Value = serde_json::from_str(
        &fixture.run(&["bulletin", "--consumer", "session:s2", "--json"], None),
    )
    .unwrap();
    assert!(
        listing["pages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["status"] == "new")
    );
}

#[test]
fn cli_mark_seen_errors_and_pre_mark_statuses() {
    let fixture = Fixture::new();
    // No consumer resolvable from the project directory: non-zero exit.
    let output = fixture.output(&["bulletin", "--mark-seen"], None, None);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no consumer"));
    let output = fixture.output(
        &["bulletin", "--consumer", "", "--mark-seen", "--json"],
        None,
        None,
    );
    assert!(!output.status.success());
    // Without a consumer, listing works and reports unknown status.
    let listing: Value = serde_json::from_str(&fixture.run(&["bulletin", "--json"], None)).unwrap();
    assert_eq!(listing["consumer"], Value::Null);
    assert_eq!(listing["pages"][0]["status"], "unknown");
    // --mark-seen --json reports statuses from before marking, plus the count.
    fixture.write("second", NOTICE);
    let args = [
        "bulletin",
        "--consumer",
        "session:s9",
        "--mark-seen",
        "--json",
    ];
    let marked: Value = serde_json::from_str(&fixture.run(&args, None)).unwrap();
    assert_eq!(marked["marked_seen"], 2);
    assert!(
        marked["pages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["status"] == "new")
    );
    let again: Value = serde_json::from_str(&fixture.run(&args, None)).unwrap();
    assert_eq!(again["marked_seen"], 0);
    assert!(
        again["pages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["status"] == "seen")
    );
    set_mtime(&fixture.0.join("wiki/pages/knowledge/second.md"), 9);
    let text = fixture.run(
        &["bulletin", "--consumer", "session:s9", "--mark-seen"],
        None,
    );
    assert!(text.contains("\nupdated  "), "{text}");
    assert!(text.ends_with("marked 1 page(s) seen\n"), "{text}");
    // Marked pages are not announced to that session.
    assert_eq!(
        fixture.run(&REMIND, prompt("s9", &fixture.0.join("project"))),
        ""
    );
}

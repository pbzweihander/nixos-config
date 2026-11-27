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
        let path = std::env::temp_dir().join(format!("wiki-codex-hooks-{}", std::process::id()));
        fs::create_dir_all(path.join("wiki/pages/knowledge")).unwrap();
        fs::write(path.join("wiki/pages/knowledge/socket.md"),
            "---\ntitle: Socket mount isolation\ncreated: 2026-01-01\n---\nerror: cannot establish app-server socket mount isolation\n").unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str], input: Value) -> String {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-wiki"));
        let mut child = crate::hermetic(&mut command)
            .args(args)
            .current_dir(&self.0)
            .env("CLAUDE_WIKI_DIR", self.0.join("wiki"))
            .env("CLAUDE_WIKI_SOCKET", "")
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
            .write_all(input.to_string().as_bytes())
            .unwrap();
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

fn context(output: &str, event: &str) -> String {
    let value: Value = serde_json::from_str(output).unwrap();
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], event);
    value["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn codex_hook_commands_accept_payloads_inject_context_and_reset_on_rollout_sync() {
    let fixture = Fixture::new();
    let transcript = fixture.0.join("rollout.jsonl");
    let mut hook = json!({"session_id":"codex-session", "transcript_path":transcript,
        "cwd":fixture.0, "hook_event_name":"SessionStart", "model":"fixture-model"});
    let start = fixture.run(&["context", "--harness", "codex"], hook.clone());
    assert!(context(&start, "SessionStart").contains("Shared wiki: 1 pages"));
    hook["hook_event_name"] = json!("UserPromptSubmit");
    hook["prompt"] = json!("");
    let remind = ["remind", "--harness", "codex", "--interval", "2"];
    assert!(fixture.run(&remind, hook.clone()).is_empty());
    let reminder = fixture.run(&remind, hook.clone());
    assert!(context(&reminder, "UserPromptSubmit").contains("2 prompts"));
    assert!(fixture.run(&remind, hook.clone()).is_empty());
    fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({"type":"response_item","payload":{
        "type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"claude-wiki sync\"}"}})
        ),
    )
    .unwrap();
    assert!(fixture.run(&remind, hook.clone()).is_empty());
    assert!(!fixture.run(&remind, hook).is_empty());

    let mut failure = json!({"session_id":"failure-session", "hook_event_name":"PostToolUse",
        "tool_name":"Bash", "tool_input":{"command":"build-app"},
        "tool_response":{"exit_code":0,"stderr":"error: cannot establish app-server socket mount isolation"}});
    let recall = ["recall", "--harness", "codex"];
    assert!(context(&fixture.run(&recall, failure.clone()), "PostToolUse").contains("error line"));
    failure["session_id"] = json!("known-failure");
    failure["tool_response"]["exit_code"] = json!(1);
    let result = fixture.run(&recall, failure.clone());
    assert!(context(&result, "PostToolUse").contains("socket.md"));
    failure["session_id"] = json!("unknown-status-session");
    failure["tool_response"] = json!("error: cannot establish app-server socket mount isolation");
    let result = fixture.run(&recall, failure.clone());
    assert!(context(&result, "PostToolUse").contains("exit status unavailable"));
    assert!(fixture.run(&recall, failure).is_empty());

    let event = ["remind", "--event", "--harness", "codex"];
    let mut commit = json!({"session_id":"commit", "hook_event_name":"PostToolUse",
        "tool_name":"functions.exec_command", "tool_input":{"cmd":"git commit -m fix"},
        "tool_response":"[main abc123] fix"});
    assert!(fixture.run(&event, commit.clone()).is_empty());
    commit["tool_response"] = json!("Process exited with code 1\nOutput:\nerror: failed");
    assert!(fixture.run(&event, commit.clone()).is_empty());
    commit["tool_response"] = json!("Process exited with code 0\nOutput:\n[main abc123] fix");
    assert!(context(&fixture.run(&event, commit.clone()), "PostToolUse").contains("committed"));
    assert!(fixture.run(&event, commit).is_empty());
    let mut debug = json!({"session_id":"debug-streak", "hook_event_name":"PostToolUse", "tool_name":"exec_command", "tool_input":{"cmd":"cargo test"}, "tool_response":{"exit_code":1}});
    for _ in 0..3 {
        assert!(fixture.run(&event, debug.clone()).is_empty());
    }
    debug["tool_response"] = json!({"exit_code":0});
    assert!(context(&fixture.run(&event, debug), "PostToolUse").contains("debugging streak"));
}

#[path = "integration/antigravity_hooks.rs"]
mod antigravity_hooks;
#[path = "integration/antigravity_sessions.rs"]
mod antigravity_sessions;
#[path = "integration/bot_guard.rs"]
mod bot_guard;
#[path = "integration/bulletin.rs"]
mod bulletin;
#[path = "integration/codex_hooks.rs"]
mod codex_hooks;
#[path = "integration/records.rs"]
mod records;
#[path = "integration/semantic.rs"]
mod semantic;

/// Drops every inherited `AGENT_WIKI_*` and `CLAUDE_WIKI_*` setting and
/// `ORCA_BOT_HOME`, so a value exported by the developer (for example
/// `AGENT_WIKI_DIR` or `AGENT_WIKI_STATE_DIR`, which win over the legacy names)
/// cannot point a fixture at another wiki or state directory. The embedding
/// socket is disabled until the fixture sets its own; set fixture values after
/// this call. Every agent-wiki subprocess in these tests is built through it.
pub fn hermetic(command: &mut std::process::Command) -> &mut std::process::Command {
    for (key, _) in std::env::vars_os() {
        if let Some(key) = key.to_str()
            && (key.starts_with("AGENT_WIKI_") || key.starts_with("CLAUDE_WIKI_"))
        {
            command.env_remove(key);
        }
    }
    command
        .env_remove("ORCA_BOT_HOME")
        .env("CLAUDE_WIKI_SOCKET", "")
}

/// Reruns one test per fixture builder in a child process whose environment
/// points every wiki setting at sentinel paths, and checks that they survive
/// untouched. The full polluted `cargo test` run covers the rest.
#[test]
fn polluted_environment_never_reaches_fixtures() {
    const CHILD: &str = "WIKI_POLLUTION_CHILD";
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("wiki-polluted-{}", std::process::id()));
    let state = dir.join("polluted-state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("sentinel"), "keep").unwrap();
    let tests = [
        "antigravity_hooks::context_uses_camelcase_session_and_workspace_fallback_and_logs_injections",
        "antigravity_sessions::default_dirs_are_searched_with_harness_labels_and_filters",
        "bot_guard::non_bot_manual_and_wrong_events_never_create_guard_state",
        "bulletin::session_start_and_prompts_announce_each_change_once_per_consumer",
        "codex_hooks::codex_hook_commands_accept_payloads_inject_context_and_reset_on_rollout_sync",
        "records::backfill_includes_orca_and_exec_and_reports_exclusion_reasons",
        "semantic::no_refresh_reads_the_existing_index_without_writing",
    ];
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "--test-threads", "1"])
        .args(tests)
        .env(CHILD, "1")
        .env("AGENT_WIKI_DIR", dir.join("polluted"))
        .env("AGENT_WIKI_STATE_DIR", &state)
        .env("AGENT_WIKI_SOCKET", dir.join("polluted.sock"))
        .env("CLAUDE_WIKI_DIR", dir.join("polluted-legacy"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    entries.sort();
    let state_entries: Vec<_> = std::fs::read_dir(&state)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    let sentinel = std::fs::read_to_string(state.join("sentinel"));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(output.status.success(), "{output:?}");
    assert!(
        stdout.contains(&format!("{} passed", tests.len())),
        "{stdout}"
    );
    assert_eq!(entries, ["polluted-state"]);
    assert_eq!(state_entries, ["sentinel"]);
    assert_eq!(sentinel.unwrap(), "keep");
}

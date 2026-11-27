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

/// Drops every inherited `AGENT_WIKI_*` and `CLAUDE_WIKI_*` setting, so a value
/// exported by the developer (for example `AGENT_WIKI_DIR`, which wins over the
/// legacy name) cannot point a fixture at another wiki. The embedding socket is
/// disabled until the fixture sets its own; set fixture values after this call.
pub fn hermetic(command: &mut std::process::Command) -> &mut std::process::Command {
    for (key, _) in std::env::vars_os() {
        if let Some(key) = key.to_str()
            && (key.starts_with("AGENT_WIKI_") || key.starts_with("CLAUDE_WIKI_"))
        {
            command.env_remove(key);
        }
    }
    command.env("CLAUDE_WIKI_SOCKET", "")
}

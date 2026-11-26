//! Shared, read-only transcript decoding. No event_msg copies or reasoning are indexed.
use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

// Kept in sync with wiki-curator/wiki_curator/prompts.py. A prompt marker
// survives relocated worktrees and does not rely on private Codex env overrides.
pub const CURATOR_MARKER: &str = "[wiki-curator automation]";

pub fn cwd_class(cwd: &str) -> &'static str {
    let path = Path::new(cwd);
    let cache = crate::env_path("XDG_CACHE_HOME").unwrap_or_else(|| crate::home().join(".cache"));
    if [cache, crate::home().join(".cache")]
        .iter()
        .any(|root| path.starts_with(root.join("wiki-curator")))
    {
        "wiki_curator"
    } else if [".claude", ".pclaude"].iter().any(|dir| {
        path.strip_prefix(crate::home().join(dir).join("jobs"))
            .ok()
            .is_some_and(|relative| {
                relative
                    .components()
                    .nth(1)
                    .is_some_and(|c| c.as_os_str() == "tmp")
            })
    }) {
        "claude_jobs_tmp"
    } else {
        "project_or_other"
    }
}

pub fn curator_reason(cwd: &str, prompt: &str) -> Option<&'static str> {
    if prompt.lines().next() == Some(CURATOR_MARKER) {
        Some("wiki_curator_marker")
    } else if cwd_class(cwd) == "wiki_curator" {
        Some("wiki_curator_cwd")
    } else {
        None
    }
}

/// Session-wide exclusions: do not leave assistant/tool rows from an excluded
/// run behind. Only the first real prompt is a launcher marker; later quotations
/// of a curator prompt during normal project work do not exclude that session.
pub fn exclusion(path: &Path, codex: Option<&Codex>) -> Result<Option<&'static str>> {
    if codex.is_some_and(|meta| meta.skipped) {
        return Ok(Some("codex_subagent"));
    }
    let mut cwd_reason = codex.and_then(|meta| curator_reason(&meta.cwd, ""));
    for line in BufReader::new(File::open(path)?).split(b'\n') {
        let Ok(value) = serde_json::from_slice::<Value>(&line?) else {
            continue;
        };
        cwd_reason =
            cwd_reason.or_else(|| curator_reason(string(&value, "cwd").unwrap_or_default(), ""));
        let prompt = if let Some(meta) = codex {
            meta.message(&value)
                .filter(|(role, _)| role == "user")
                .map(|(_, text)| text)
        } else if value["type"] == "user" && !skipped(&value) {
            Some(text(&value["message"]["content"]))
        } else {
            None
        };
        if let Some(prompt) = prompt.filter(|p| !p.trim().is_empty() && !p.starts_with('<')) {
            return Ok(curator_reason("", &prompt).or(cwd_reason));
        }
    }
    Ok(cwd_reason)
}

#[derive(Default)]
pub struct Codex {
    pub session: String,
    pub cwd: String,
    pub branch: String,
    pub originator: String,
    pub source: String,
    pub skipped: bool,
}

impl Codex {
    pub fn open(path: &Path) -> Result<Option<Self>> {
        let mut line = String::new();
        BufReader::new(File::open(path)?).read_line(&mut line)?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            return Ok(None);
        };
        if value["type"] != "session_meta" {
            return Ok(None);
        }
        let p = &value["payload"];
        Ok(Some(Self {
            session: string(p, "id")
                .or_else(|| string(p, "session_id"))
                .unwrap_or_default()
                .into(),
            cwd: string(p, "cwd").unwrap_or_default().into(),
            branch: string(&p["git"], "branch").unwrap_or_default().into(),
            originator: string(p, "originator").unwrap_or_default().into(),
            source: match &p["source"] {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                source => source.to_string(),
            },
            // Orca desktop and codex_exec sessions include delegated real work.
            // The exec runner alone does not prove a scripted one-shot; preserve
            // its provenance instead. Curator automation is checked separately
            // by the session-wide exclusion above.
            skipped: p["source"].get("subagent").is_some(),
        }))
    }

    pub fn message(&self, value: &Value) -> Option<(String, String)> {
        if self.skipped || value["type"] != "response_item" {
            return None;
        }
        let p = &value["payload"];
        let role = p["role"].as_str()?;
        if p["type"] != "message" || !["user", "assistant"].contains(&role) {
            return None;
        }
        let text = text(&p["content"]);
        let text = if role == "user" {
            text.split_once("<USER_REQUEST>")
                .and_then(|(_, tail)| tail.split_once("</USER_REQUEST>"))
                .map_or(text.clone(), |(prompt, _)| prompt.trim().to_owned())
        } else {
            text
        };
        // Environment/AGENTS instructions are user-role wrappers, not prompts.
        if text.trim().is_empty()
            || (role == "user"
                && ([
                    "<environment_context>",
                    "<permissions instructions>",
                    "# AGENTS.md instructions",
                ]
                .iter()
                .any(|prefix| text.starts_with(prefix))))
        {
            return None;
        }
        Some((role.into(), text))
    }
}

pub fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

pub fn timestamp(value: &Value) -> Option<String> {
    Some(
        DateTime::parse_from_rfc3339(value["timestamp"].as_str()?)
            .ok()?
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

pub fn skipped(value: &Value) -> bool {
    skip_reason(value).is_some()
}

pub fn skip_reason(value: &Value) -> Option<&'static str> {
    if value["entrypoint"] == "sdk-cli" {
        Some("claude_sdk_cli")
    } else if value["isSidechain"] == true {
        Some("claude_sidechain")
    } else if value["isMeta"] == true {
        Some("claude_meta")
    } else {
        None
    }
}

pub fn harness_label<'a>(harness: &'a str, originator: &str) -> &'a str {
    if harness == "codex" && originator == "orca_desktop" {
        "codex/orca"
    } else {
        harness
    }
}

pub fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| {
                matches!(
                    p["type"].as_str(),
                    Some("text" | "input_text" | "output_text")
                )
            })
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_keeps_wrapped_requests_and_ordinary_markup() {
        let codex = Codex::default();
        for (input, expected) in [
            ("<USER_REQUEST>fix this</USER_REQUEST>", Some("fix this")),
            ("<xml> is broken", Some("<xml> is broken")),
            ("<environment_context>cwd</environment_context>", None),
        ] {
            let value = serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":input}]}});
            assert_eq!(
                codex.message(&value).map(|(_, text)| text),
                expected.map(str::to_owned)
            );
        }
    }
}

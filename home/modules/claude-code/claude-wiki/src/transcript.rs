//! Shared, read-only transcript decoding. No event_msg copies or reasoning are indexed.
use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

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
            // its provenance instead. Only explicit sidechains are excluded.
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
        // Environment/AGENTS instructions are user-role wrappers, not prompts.
        if text.trim().is_empty()
            || (role == "user"
                && (text.starts_with('<') || text.starts_with("# AGENTS.md instructions")))
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

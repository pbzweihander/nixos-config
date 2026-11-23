//! Shared hook wire format and the small rollout subset used by wiki hooks.
use clap::ValueEnum;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::LazyLock;

#[derive(Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Harness {
    #[default]
    Claude,
    Codex,
}

impl Harness {
    pub fn context(self, event: &str, text: &str) -> String {
        match self {
            Self::Claude => text.to_owned(),
            Self::Codex => additional_context(event, text).to_string(),
        }
    }
}

pub fn additional_context(event: &str, text: &str) -> Value {
    json!({"hookSpecificOutput": {"hookEventName": event, "additionalContext": text}})
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Hook {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub prompt: Option<String>,
    pub hook_event_name: String,
    pub tool_name: String,
    pub tool_input: Value,
    pub tool_response: Value,
    pub error: Value,
    pub is_interrupt: bool,
}

pub fn shell_tool(name: &str) -> bool {
    matches!(
        name.rsplit('.').next().unwrap_or(name),
        "Bash" | "exec_command" | "shell" | "shell_command"
    )
}

impl Hook {
    pub fn response_text(&self) -> String {
        self.tool_response
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                ["stderr", "stdout", "output", "aggregated_output"]
                    .into_iter()
                    .filter_map(|key| self.tool_response[key].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    }

    pub fn exit_code(&self) -> Option<i64> {
        self.tool_response["exit_code"]
            .as_i64()
            .or_else(|| self.tool_response["metadata"]["exit_code"].as_i64())
            .or_else(|| exit_status(&self.response_text()))
    }

    pub fn normalize_shell(&mut self) {
        if shell_tool(&self.tool_name) {
            self.tool_name = "Bash".into();
            if self.tool_input["command"].is_null() {
                self.tool_input["command"] = self.tool_input["cmd"].clone();
            }
        }
    }

    pub fn codex_failure(&self) -> Option<ShellFailure> {
        if self.hook_event_name != "PostToolUse"
            || !shell_tool(&self.tool_name)
            || self.is_interrupt
        {
            return None;
        }
        let text = self.response_text();
        let code = self.exit_code();
        if code == Some(0) {
            return None;
        }
        if code.is_some() {
            return Some(ShellFailure {
                text,
                uncertain: false,
            });
        }
        // 0.155.1's Bash hook response is raw output with no exit status, even
        // though the model-facing exec output has a status header. Diagnostic
        // lines are a conservative fallback, not proof of failure: e.g. a command
        // can successfully print a captured error. Recall separately deduplicates
        // normalized lines and limits this fallback to three emissions per session.
        let lines = diagnostic_lines(&text);
        (!lines.is_empty()).then(|| ShellFailure {
            text: lines.join("\n"),
            uncertain: true,
        })
    }
}

pub struct ShellFailure {
    pub text: String,
    pub uncertain: bool,
}

fn exit_status(text: &str) -> Option<i64> {
    text.lines()
        .take(12)
        .take_while(|line| line.trim() != "Output:")
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("Exit code: ")
                .or_else(|| line.strip_prefix("Process exited with code "))?
                .trim()
                .parse()
                .ok()
        })
}

pub fn diagnostic_lines(text: &str) -> Vec<String> {
    static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
    static DIAGNOSTIC: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?ix)^(?:
            (?:error(?:\[[a-z0-9_-]+\])?|fatal(?:\s+error)?|panic|exception)\s*:
            | [a-z_.]*(?:error|exception)\s*:
            | (?:[a-z0-9_./+-]+):\s+(?:error\b|fatal\b|cannot\b|can't\b|permission\s+denied\b|no\s+such\b|command\s+not\s+found\b)
            | thread\s+'[^']+'\s+panicked\s+at\b
            | traceback\s+\(most\s+recent\s+call\s+last\):
        )").unwrap()
    });
    text.lines()
        .rev()
        .take(200)
        .filter_map(|line| {
            let line = ANSI.replace_all(line, "");
            let line = line.trim();
            (line.chars().count() <= 2000 && DIAGNOSTIC.is_match(line)).then(|| line.to_owned())
        })
        .take(20)
        .collect()
}

pub fn ran_sync(line: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(line) else {
        return false;
    };
    if v["type"] == "assistant" {
        return v["message"]["content"].as_array().is_some_and(|parts| {
            parts.iter().any(|p| {
                p["type"] == "tool_use"
                    && p["input"]["command"]
                        .as_str()
                        .is_some_and(crate::compat::sync_command)
            })
        });
    }
    let item = &v["payload"];
    if v["type"] != "response_item"
        || item["type"] != "function_call"
        || !item["name"].as_str().is_some_and(shell_tool)
    {
        return false;
    }
    let Some(arguments) = item["arguments"].as_str() else {
        return false;
    };
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return false;
    };
    [&args["cmd"], &args["command"]].iter().any(|command| {
        command.as_str().is_some_and(crate::compat::sync_command)
            || command.as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .any(|p| p.as_str().is_some_and(crate::compat::sync_command))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_one_json_envelope_and_claude_stays_plain() {
        let text = "wiki\n\"quoted\" 한국어";
        let output: Value =
            serde_json::from_str(&Harness::Codex.context("SessionStart", text)).unwrap();
        assert_eq!(output["hookSpecificOutput"]["additionalContext"], text);
        assert_eq!(
            output["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        assert_eq!(Harness::Claude.context("SessionStart", text), text);
    }

    #[test]
    fn codex_payload_requires_a_failed_shell() {
        let mut hook: Hook = serde_json::from_value(json!({
            "session_id":"codex-session", "transcript_path":null,
            "hook_event_name":"PostToolUse", "tool_name":"Bash",
            "tool_input":{"command":"false"},
            "tool_response":{"exit_code":1,"stdout":"","stderr":"error: unavailable"}
        }))
        .unwrap();
        assert!(
            hook.codex_failure()
                .unwrap()
                .text
                .contains("error: unavailable")
        );
        hook.tool_response["exit_code"] = json!(0);
        assert!(hook.codex_failure().is_none());
        hook.tool_response["exit_code"] = Value::Null;
        assert!(hook.codex_failure().unwrap().uncertain);
        hook.tool_response["exit_code"] = json!(1);
        hook.tool_name = "apply_patch".into();
        assert!(hook.codex_failure().is_none());
        hook.tool_name = "Bash".into();
        hook.is_interrupt = true;
        assert!(hook.codex_failure().is_none());
    }

    #[test]
    fn status_headers_override_diagnostics_and_output_status_lookalikes_do_not() {
        let mut hook = Hook {
            hook_event_name: "PostToolUse".into(),
            tool_name: "Bash".into(),
            ..Hook::default()
        };
        for header in ["Exit code: ", "Process exited with code "] {
            hook.tool_response = json!(format!("{header}0\nOutput:\nerror: unavailable"));
            assert!(hook.codex_failure().is_none());
            hook.tool_response = json!(format!("{header}1\nOutput:\nerror: unavailable"));
            assert!(!hook.codex_failure().unwrap().uncertain);
        }
        hook.tool_response =
            json!({"exit_code":0,"output":"Exit code: 1\nOutput:\nerror: unavailable"});
        assert!(hook.codex_failure().is_none());
        hook.tool_response = json!("Output:\nExit code: 1\nlet error = missing;");
        assert!(hook.codex_failure().is_none());
        for text in [
            "fn error() { panic!(\"failure\"); }",
            "src/main.rs:12: error: cannot open file",
            "    let msg = \"error: cannot open file\";",
            "# This error occurs when the socket is missing",
            "  \"error\": \"cannot open file\",",
        ] {
            hook.tool_response = json!(text);
            assert!(hook.codex_failure().is_none(), "{text}");
        }
        for text in [
            "\x1b[31merror: cannot open file\x1b[0m",
            "bash: cannot open file",
            "RuntimeError: cannot open file",
        ] {
            hook.tool_response = json!(text);
            assert!(hook.codex_failure().unwrap().uncertain);
        }
    }

    #[test]
    fn rollout_sync_only_counts_shell_function_calls() {
        for (name, args) in [
            (
                "exec_command",
                json!({"cmd":"agent-wiki sync pages/knowledge/x.md"}),
            ),
            (
                "functions.shell",
                json!({"command":["bash","-c","agent-wiki sync"]}),
            ),
        ] {
            let mut line = json!({"type":"response_item","payload":{
                "type":"function_call","name":name,"arguments":args.to_string()}});
            assert!(ran_sync(line.to_string().as_bytes()));
            line["payload"]["name"] = json!("search");
            assert!(!ran_sync(line.to_string().as_bytes()));
        }
        assert!(!ran_sync(br#"{"type":"event_msg","payload":{"type":"user_message","message":"agent-wiki sync"}}"#));
        assert!(!ran_sync(b"partial JSON"));
    }
}

//! Permanent compatibility for old settings and transcript paths.
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

pub fn var_os(primary: &str) -> Option<OsString> {
    env::var_os(primary).or_else(|| env::var_os(primary.replacen("AGENT_WIKI_", "CLAUDE_WIKI_", 1)))
}

pub fn var(primary: &str) -> Result<String, env::VarError> {
    match var_os(primary) {
        Some(value) => value.into_string().map_err(env::VarError::NotUnicode),
        None => Err(env::VarError::NotPresent),
    }
}

// Clap supports only one env name. Choose before parsing; an explicitly empty
// primary value still takes precedence, just as it does for socket disabling.
pub fn env_name(primary: &'static str, legacy: &'static str) -> &'static str {
    if env::var_os(primary).is_none() && env::var_os(legacy).is_some() {
        legacy
    } else {
        primary
    }
}

pub fn directory(base: &Path) -> PathBuf {
    let new = base.join("agent-wiki");
    let old = base.join("claude-wiki");
    if !new.exists() && old.exists() {
        old
    } else {
        new
    }
}

pub fn roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.to_path_buf()];
    // Old transcripts keep literal paths even after migration. Do not require
    // their targets to exist (deleted pages must still count as writes).
    if let Some(parent) = root.parent() {
        match root.file_name().and_then(|s| s.to_str()) {
            Some("agent-wiki") => roots.push(parent.join("claude-wiki")),
            Some("claude-wiki") => roots.push(parent.join("agent-wiki")),
            _ => {}
        }
    }
    roots
}

pub fn wiki_skill(name: &str) -> bool {
    matches!(name, "agent-wiki" | "claude-wiki")
}

pub fn sync_command(command: &str) -> bool {
    use crate::usage::{ShellToken, shell_tokens};
    shell_tokens(command)
        .split(|t| matches!(t, ShellToken::Op(";" | "&&" | "||" | "|" | "&" | "(" | ")")))
        .any(|segment| {
            let words: Vec<_> = segment
                .iter()
                .filter_map(|t| {
                    if let ShellToken::Word(word) = t {
                        Some(word.as_str())
                    } else {
                        None
                    }
                })
                .collect();
            let words = words.as_slice();
            let start = words
                .iter()
                .position(|word| !word.contains('='))
                .unwrap_or(words.len());
            let mut words = &words[start..];
            if words
                .first()
                .is_some_and(|word| matches!(*word, "env" | "command" | "exec"))
            {
                words = &words[1..];
                while words
                    .first()
                    .is_some_and(|word| word.starts_with('-') || word.contains('='))
                {
                    words = &words[1..];
                }
            }
            words.len() >= 2
                && words[1] == "sync"
                && Path::new(words[0])
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(wiki_skill)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_requires_an_executed_command() {
        for command in [
            "agent-wiki sync",
            "cd /tmp && /bin/claude-wiki sync -m x",
            "env X=y agent-wiki sync",
            "X=y command agent-wiki sync",
        ] {
            assert!(sync_command(command), "{command}");
        }
        for command in [
            "echo 'agent-wiki sync'",
            "rg 'agent-wiki sync'",
            "printf '%s' 'agent-wiki sync'",
            "echo agent-wiki sync",
            "cat agent-wiki sync",
        ] {
            assert!(!sync_command(command), "{command}");
        }
    }
    #[test]
    fn historical_paths_remain_attributable_without_existing_files() {
        for active in ["agent-wiki", "claude-wiki"] {
            let root = PathBuf::from("/home/test/.local/share").join(active);
            for name in ["agent-wiki", "claude-wiki"] {
                let path = format!("/home/test/.local/share/{name}/pages/knowledge/deleted.md");
                assert_eq!(
                    crate::usage::page_path(&root, &path, Path::new("/")),
                    Some("pages/knowledge/deleted.md".into())
                );
                assert_eq!(
                    crate::usage::bash_accesses(
                        &root,
                        &format!("cat {path}; echo fix >> {path}"),
                        Path::new("/")
                    )
                    .len(),
                    2
                );
            }
        }
    }
}

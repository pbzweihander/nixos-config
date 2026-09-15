use anyhow::Result;
use regex::Regex;
use serde_yaml_ng::{Mapping, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub const TYPES: [&str; 4] = ["knowledge", "projects", "followups", "history"];
static FRONTMATTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\A---\n(.*?)\n---\n?").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#\s+(.+)$").unwrap());

#[derive(Default, Debug)]
pub struct Page {
    pub kind: String,
    pub status: String,
    pub title: String,
    pub tags: String,
    pub project: String,
    pub created: String,
    pub body: String,
    pub problems: Vec<String>,
}

pub fn page_type(rel: &str) -> &str {
    let parts: Vec<_> = rel.split('/').collect();
    if parts.len() == 3 && parts[0] == "pages" && TYPES.contains(&parts[1]) {
        parts[1]
    } else {
        ""
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        _ => serde_yaml_ng::to_string(v)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    }
}

pub fn parse(text: &str, rel: &str) -> Page {
    let mut page = Page {
        kind: page_type(rel).into(),
        body: text.into(),
        ..Page::default()
    };
    let mut meta = Mapping::new();
    if let Some(m) = FRONTMATTER.captures(text) {
        page.body = text[m.get(0).unwrap().end()..].into();
        match serde_yaml_ng::from_str::<Value>(&m[1]) {
            Ok(value @ (Value::Mapping(_) | Value::Null)) => {
                if let Value::Mapping(mapping) = value {
                    meta = mapping;
                }
                let mut required = vec!["title", "created"];
                if page.kind == "followups" {
                    required.push("status");
                }
                for key in required {
                    if meta
                        .get(Value::String(key.into()))
                        .is_none_or(|v| scalar(v).is_empty())
                    {
                        page.problems
                            .push(format!("has no {key} in its frontmatter"));
                    }
                }
            }
            Ok(_) => page
                .problems
                .push("has frontmatter that is not a YAML mapping".into()),
            Err(e) => page.problems.push(format!(
                "has invalid YAML frontmatter: {}",
                e.to_string()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
        }
    } else {
        page.problems.push("has no frontmatter".into());
    }
    let get = |key: &str| {
        meta.get(Value::String(key.into()))
            .map(scalar)
            .unwrap_or_default()
    };
    page.status = get("status");
    if !page.status.is_empty() && !["open", "done"].contains(&page.status.as_str()) {
        page.problems.push(format!(
            "has status '{}'; use one of open, done",
            page.status
        ));
    }
    let tags: Vec<String> = match meta.get(Value::String("tags".into())) {
        Some(Value::String(s)) => s.split(',').map(str::to_owned).collect(),
        Some(Value::Sequence(s)) => s.iter().map(scalar).collect(),
        Some(v) => vec![scalar(v)],
        None => vec![],
    };
    page.tags = tags
        .iter()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join("-"))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let stem = Path::new(rel)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    page.title = get("title");
    if page.title.is_empty() {
        page.title = HEADING
            .captures(&page.body)
            .map(|c| c[1].trim().to_owned())
            .unwrap_or_else(|| stem.to_string());
    }
    page.project = get("project");
    if page.project.is_empty() && page.kind == "projects" {
        page.project = stem.to_string();
    }
    page.created = get("created");
    page
}

pub fn read(path: &Path) -> Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(path)?)
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

pub fn markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = vec![];
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            paths.extend(markdown_files(&entry.path())?);
        } else if entry.path().extension().is_some_and(|s| s == "md") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frontmatter_and_fallbacks() {
        let p = parse(
            "---\ntitle: Test\ncreated: 2026-09-16\ntags:\n - two words\n - rust\n---\nBody\n",
            "pages/projects/demo.md",
        );
        assert_eq!(p.tags, "two-words rust");
        assert_eq!(p.project, "demo");
        assert_eq!(p.body, "Body\n");
        assert!(p.problems.is_empty());
        let p = parse(
            "---\ntitle: [bad\n---\n# Fallback\n",
            "pages/followups/x.md",
        );
        assert_eq!(p.title, "Fallback");
        assert!(p.problems[0].contains("invalid YAML"));
        assert_eq!(
            parse("---\n[]\n---\nbody", "x.md").problems,
            ["has frontmatter that is not a YAML mapping"]
        );
    }
}

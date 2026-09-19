use anyhow::Result;
use regex::Regex;
use serde_yaml_ng::{Mapping, Value};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub const TYPES: [&str; 4] = ["knowledge", "projects", "followups", "history"];
static FRONTMATTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\A---\n(.*?)\n---\n?").unwrap());
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#\s+(.+)$").unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\[\]\r\n]*)\]\]").unwrap());
static TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A([a-z]+)/([a-z0-9._-]+)\z").unwrap());

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
    pub links: Vec<String>,
}

pub fn link_path(target: &str) -> Option<String> {
    let captures = TARGET.captures(target)?;
    TYPES
        .contains(&&captures[1])
        .then(|| format!("pages/{target}.md"))
}

fn parse_links(body: &str, rel: &str) -> Vec<String> {
    let mut prose = String::new();
    let mut fence = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let marker = trimmed.as_bytes().first().copied().unwrap_or_default();
        let run = trimmed.bytes().take_while(|b| *b == marker).count();
        if let Some((opening, length)) = fence {
            if marker == opening && run >= length && trimmed[run..].trim().is_empty() {
                fence = None;
            }
            // Keep separated prose from accidentally forming a link across a fence.
            prose.push('\n');
        } else if matches!(marker, b'`' | b'~') && run >= 3 {
            fence = Some((marker, run));
            prose.push('\n');
        } else {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    // Code spans close only with a backtick run of the same length. Unmatched
    // backticks remain prose, and spans may cross line boundaries.
    let mut visible = String::new();
    let mut rest = prose.as_str();
    while let Some(start) = rest.find('`') {
        visible.push_str(&rest[..start]);
        rest = &rest[start..];
        let length = rest.bytes().take_while(|b| *b == b'`').count();
        let mut end = length;
        let mut closing = None;
        while let Some(next) = rest[end..].find('`') {
            let next = end + next;
            let run = rest[next..].bytes().take_while(|b| *b == b'`').count();
            end = next + run;
            if run == length {
                closing = Some(end);
                break;
            }
        }
        if let Some(end) = closing {
            visible.push('\n');
            rest = &rest[end..];
        } else {
            visible.push_str(&rest[..length]);
            rest = &rest[length..];
        }
    }
    visible.push_str(rest);
    let mut seen = HashSet::new();
    LINK.captures_iter(&visible)
        .filter_map(|c| link_path(c[1].split('|').next().unwrap()))
        .filter(|path| path != rel && seen.insert(path.clone()))
        .collect()
}

/// One markdown heading's section: its breadcrumb, its 1-based line range in the file,
/// and its text. A page with no headings has a single section with an empty breadcrumb.
#[derive(Debug, PartialEq)]
pub struct Section {
    pub heading: String,
    pub start: i64,
    pub end: i64,
    pub body: String,
}

pub fn sections(text: &str) -> Vec<Section> {
    static HEADING_LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\A(#{1,6})\s+(.+?)\s*#*\s*\z").unwrap());
    let mut sections: Vec<Section> = vec![];
    // The breadcrumb keeps one title per heading level, so a deep heading reads as
    // "Modes and cache > Diagnose" rather than losing its parent.
    let mut crumbs: Vec<String> = vec![];
    let mut fence: Option<(u8, usize)> = None;
    for (i, line) in text.lines().enumerate() {
        let number = i as i64 + 1;
        let trimmed = line.trim_start();
        let marker = trimmed.as_bytes().first().copied().unwrap_or_default();
        let run = trimmed.bytes().take_while(|b| *b == marker).count();
        if let Some((opening, length)) = fence {
            if marker == opening && run >= length && trimmed[run..].trim().is_empty() {
                fence = None;
            }
        } else if matches!(marker, b'`' | b'~') && run >= 3 {
            fence = Some((marker, run));
        } else if let Some(m) = HEADING_LINE.captures(trimmed) {
            let level = m[1].len();
            crumbs.truncate(level - 1);
            crumbs.resize(level - 1, String::new());
            crumbs.push(m[2].to_owned());
            if let Some(last) = sections.last_mut() {
                last.end = number - 1;
            }
            sections.push(Section {
                heading: crumbs
                    .iter()
                    .filter(|c| !c.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" > "),
                start: number,
                end: number,
                body: String::new(),
            });
            continue;
        }
        match sections.last_mut() {
            Some(section) => {
                section.body.push_str(line);
                section.body.push('\n');
                section.end = number;
            }
            None => {
                sections.push(Section {
                    heading: String::new(),
                    start: number,
                    end: number,
                    body: format!("{line}\n"),
                });
            }
        }
    }
    sections
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
    page.links = parse_links(&page.body, rel);
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
    fn links_resolve_in_first_appearance_order() {
        let page = parse(
            "---\ntitle: '[[knowledge/frontmatter]]'\ncreated: 2026-09-16\n---\n\
             [[knowledge/foo|the foo page]] [[projects/demo]] [[followups/a_1.2-b]]\n\
             [[history/2026-09-16-x]] [[knowledge/foo]] [[knowledge/self]]\n\
             [[invalid/foo]] [[knowledge/Upper]] [[knowledge/a/b]] [[knowledge/]]\n\
             [[knowledge/a b]] [[knowledge/foo#anchor]] [[ knowledge/foo]]",
            "pages/knowledge/self.md",
        );
        assert_eq!(
            page.links,
            [
                "pages/knowledge/foo.md",
                "pages/projects/demo.md",
                "pages/followups/a_1.2-b.md",
                "pages/history/2026-09-16-x.md",
            ]
        );
    }

    #[test]
    fn links_exclude_code() {
        let body = "[[knowledge/before]] `[[knowledge/inline]]`\n\
                    ``a ` [[knowledge/double]]``\n\
                    ```markdown\n[[knowledge/fenced]]\n```\n\
                    ````\n```\n[[knowledge/long-fence]]\n````\n\
                    ~~~\n[[knowledge/tilde-fence]]\n~~~\n\
                    `multiline\n[[knowledge/multiline]]`\n\
                    [[knowledge/after]] `unmatched [[knowledge/unmatched]]\n\
                    ```\n[[knowledge/unclosed-fence]]";
        assert_eq!(
            parse_links(body, ""),
            [
                "pages/knowledge/before.md",
                "pages/knowledge/after.md",
                "pages/knowledge/unmatched.md",
            ]
        );
    }

    #[test]
    fn sections_track_breadcrumbs_lines_and_fences() {
        let page = "---\ntitle: t\n---\nintro\n# One\ntext\n## Two\n```\n# not a heading\n```\n### Three\nlast\n";
        let got: Vec<_> = sections(page)
            .into_iter()
            .map(|s| (s.heading, s.start, s.end))
            .collect();
        assert_eq!(
            got,
            [
                (String::new(), 1, 4),
                ("One".into(), 5, 6),
                ("One > Two".into(), 7, 10),
                ("One > Two > Three".into(), 11, 12),
            ]
        );
        // A page without headings is one section covering the file.
        let single = sections("just\ntext\n");
        assert_eq!(single.len(), 1);
        assert_eq!((single[0].start, single[0].end), (1, 2));
        assert_eq!(single[0].body, "just\ntext\n");
    }

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

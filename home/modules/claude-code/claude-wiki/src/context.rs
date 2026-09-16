use crate::{
    index::{Row, COLUMNS, JOIN},
    pages, search,
};
use anyhow::Result;
use clap::Args as ClapArgs;
use rusqlite::{params_from_iter, types::Value, Connection, OptionalExtension};
use std::{collections::HashSet, env, path::Path, process::Command};

#[derive(ClapArgs)]
pub struct Args {
    #[arg(short, default_value_t = 10)]
    pub n: usize,
    #[arg(long, env = "CLAUDE_WIKI_CONTEXT_BUDGET", default_value_t = 6000)]
    pub budget: usize,
    #[arg(long, env = "CLAUDE_WIKI_OVERVIEW_BUDGET", default_value_t = 3000)]
    pub overview_budget: usize,
}

pub fn overview_budget() -> usize {
    env::var("CLAUDE_WIKI_OVERVIEW_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000)
}

pub fn current_project() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let common = String::from_utf8_lossy(&out.stdout);
    let path = Path::new(common.trim());
    if path.file_name()? == ".git" {
        Some(path.parent()?.file_name()?.to_string_lossy().into_owned())
    } else {
        let name = path.file_name()?.to_string_lossy();
        Some(name.strip_suffix(".git").unwrap_or(&name).to_owned())
    }
}

pub fn number(n: usize) -> String {
    let s = n.to_string();
    s.chars()
        .enumerate()
        .fold(String::new(), |mut out, (i, c)| {
            if i != 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
            out
        })
}

fn cap(body: &str, budget: usize) -> (String, usize) {
    let total = body.chars().count();
    if total <= budget {
        return (body.into(), 0);
    }
    let prefix: String = body.chars().take(budget).collect();
    // A single long line still needs a bounded preview.
    let shown = prefix
        .rfind('\n')
        .map_or(prefix.as_str(), |i| &prefix[..i])
        .to_owned();
    let remaining = total - shown.chars().count();
    (shown, remaining)
}

struct Section {
    name: &'static str,
    heading: String,
    items: Vec<String>,
    original: usize,
}

fn assemble(pinned: &str, sections: &[Section], budget: usize) -> (String, usize) {
    let mut text = pinned.to_owned();
    for section in sections {
        if section.items.is_empty() {
            continue;
        }
        text.push('\n');
        if section.name == "tags" {
            text.push_str(&format!("Tags: {}\n", section.items.join(", ")));
        } else {
            text.push_str(&format!(
                "{}\n{}\n",
                section.heading,
                section.items.join("\n")
            ));
        }
    }
    let trimmed = sections
        .iter()
        .filter(|s| s.items.len() < s.original)
        .map(|s| {
            if s.items.is_empty() {
                s.name.to_owned()
            } else {
                format!(
                    "{} ({} of {})",
                    s.name,
                    s.original - s.items.len(),
                    s.original
                )
            }
        })
        .collect::<Vec<_>>();
    let suffix = if trimmed.is_empty() {
        String::new()
    } else {
        format!("; trimmed: {}", trimmed.join(", "))
    };
    // Include the footer itself and its newline in the advertised character count.
    let base = text.chars().count();
    let mut used = base;
    loop {
        let footer = format!(
            "[context {}/{} chars{suffix}]\n",
            number(used),
            number(budget)
        );
        let next = base + footer.chars().count();
        if next == used {
            text.push_str(&footer);
            return (text, used);
        }
        used = next;
    }
}

fn budgeted(pinned: &str, sections: &mut [Section], budget: usize) -> (String, usize) {
    loop {
        let (text, used) = assemble(pinned, sections, budget);
        if used <= budget {
            return (text, used);
        }
        if let Some(section) = sections.iter_mut().rev().find(|s| !s.items.is_empty()) {
            section.items.pop();
        } else {
            return (text, used);
        }
    }
}

pub fn run(root: &Path, db: &Connection, args: &Args) -> Result<()> {
    let total: i64 = db.query_row("select count(*) from files", [], |r| r.get(0))?;
    let mut pinned = format!("Shared wiki: {total} pages under {}/. Search it with `claude-wiki search <terms>` before non-trivial work; load the claude-wiki skill before writing to it.\n", root.display());
    let project = current_project();
    let overview = format!("pages/projects/{}.md", project.as_deref().unwrap_or("None"));
    let mut queries: Vec<(&str, String, String, Vec<Value>)> = vec![];
    if let Some(project) = &project {
        pinned += &format!("Current project: {project}\n");
        let row = db
            .query_row(
                &format!("select {COLUMNS} {JOIN} where fts.path = ?"),
                [&overview],
                Row::read,
            )
            .optional()?;
        if let Some(row) = row {
            if !row.blocked.is_empty() {
                pinned += &format!("\n{}\n", row.blocked_line(&overview));
            } else {
                let page = pages::parse(&pages::read(&root.join(&overview))?, &overview);
                let (shown, remaining) = cap(page.body.trim(), args.overview_budget);
                pinned += &format!(
                    "\nProject overview ({overview}) [{}/{} chars]\n{shown}\n",
                    number(shown.chars().count()),
                    number(args.overview_budget)
                );
                if remaining > 0 {
                    pinned += &format!(
                        "[... {} more chars in the page; shorten the page to fit]\n",
                        number(remaining)
                    );
                }
            }
        } else {
            pinned += &format!("No overview page for {project} yet; write {overview} once you have explored the project.\n");
        }
        queries.push((
            "follow-ups",
            format!("Open follow-ups for {project}:"),
            "fts.type = 'followups' and fts.status = 'open' and fts.project = ?".into(),
            vec![project.clone().into()],
        ));
        queries.push((
            "knowledge",
            format!("Knowledge pages for {project}:"),
            "fts.type = 'knowledge' and fts.project = ?".into(),
            vec![project.clone().into()],
        ));
    }
    queries.push((
        "recently updated",
        "Recently updated:".into(),
        "not (fts.type = 'followups' and fts.status = 'done')".into(),
        vec![],
    ));
    let mut seen = HashSet::from([overview]);
    let mut sections = vec![];
    for (name, heading, where_sql, mut params) in queries {
        params.push(Value::Integer(if name == "recently updated" {
            args.n.saturating_mul(3)
        } else {
            args.n
        } as i64));
        let mut stmt = db.prepare(&format!(
            "select {COLUMNS} {JOIN} where {where_sql} order by files.mtime_ns desc limit ?"
        ))?;
        let rows = stmt
            .query_map(params_from_iter(params), Row::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut items = vec![];
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|r| !seen.contains(&r.path))
            .take(args.n)
            .collect();
        for row in rows {
            seen.insert(row.path.clone());
            if !row.blocked.is_empty() {
                items.push(row.blocked_line(&row.path));
            } else {
                let mut extra = String::new();
                for x in [
                    row.status.as_str(),
                    if Some(&row.project) != project.as_ref() {
                        row.project.as_str()
                    } else {
                        ""
                    },
                ] {
                    if !x.is_empty() {
                        extra += &format!(", {x}");
                    }
                }
                if row.backlinks > 0 && matches!(row.kind.as_str(), "knowledge" | "followups") {
                    extra += &format!(", linked by {}", row.backlinks);
                }
                items.push(format!(
                    "- {}: {} ({}, {}{extra})",
                    row.path,
                    row.title,
                    row.kind,
                    search::updated(row.mtime)
                ));
            }
        }
        sections.push(Section {
            name,
            heading,
            original: items.len(),
            items,
        });
    }
    let items = search::tag_counts(db)?
        .into_iter()
        .take(30)
        .map(|(tag, n)| format!("{tag} ({n})"))
        .collect::<Vec<_>>();
    sections.push(Section {
        name: "tags",
        heading: String::new(),
        original: items.len(),
        items,
    });
    let (text, used) = budgeted(&pinned, &mut sections, args.budget);
    if used > args.budget {
        eprintln!("warning: context is {used} chars; protected header and overview exceed the {} char budget", args.budget);
    }
    print!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_and_line_boundaries() {
        assert_eq!(cap("abc\ndefgh", 7), ("abc".into(), 6));
        assert_eq!(cap("한글문장", 2), ("한글".into(), 2));
        assert_eq!(number(1234567), "1,234,567");
    }
    #[test]
    fn budget_counts_footer_and_drops_low_priority_first() {
        let mut sections = vec![
            Section {
                name: "follow-ups",
                heading: "Follow-ups:".into(),
                original: 1,
                items: vec!["Important".into()],
            },
            Section {
                name: "tags",
                heading: String::new(),
                original: 1,
                items: vec!["x".repeat(200)],
            },
        ];
        let (text, used) = budgeted("Header\n", &mut sections, 100);
        assert!(text.contains("Important"));
        assert!(text.contains("trimmed: tags"));
        assert_eq!(text.chars().count(), used);
        assert!(used <= 100);
    }
}

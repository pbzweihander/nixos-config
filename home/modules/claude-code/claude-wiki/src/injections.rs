use crate::index::{Row, COLUMNS, JOIN};
use anyhow::Result;
use clap::Args as ClapArgs;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(ClapArgs)]
pub struct Args {
    #[arg(long)]
    pub session: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct Page {
    path: PathBuf,
    rel: String,
    title: Option<String>,
    sources: Vec<String>,
    at: i64,
}

#[derive(Debug, Serialize)]
struct Report {
    pages: Vec<Page>,
}

fn report(root: &Path, index: &Connection, session: &str) -> Result<Report> {
    let mut pages: Vec<Page> = vec![];
    let usage_path = root.join("usage.sqlite");
    if !usage_path.exists() {
        return Ok(Report { pages });
    }
    let usage = Connection::open_with_flags(usage_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    usage.busy_timeout(Duration::from_millis(50))?;
    let root = root.canonicalize()?;
    let mut positions = HashMap::<String, usize>::new();
    let mut stmt = usage.prepare(
        "select path, source, max(at) from injections where session_id=?
         group by path, source order by max(at) desc, path, source",
    )?;
    let rows = stmt.query_map([session], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (rel, source, at) = row?;
        if let Some(&position) = positions.get(&rel) {
            pages[position].sources.push(source);
        } else {
            let title = index
                .query_row(
                    &format!("select {COLUMNS} {JOIN} where files.path=?"),
                    [&rel],
                    Row::read,
                )
                .optional()?
                .map(|row| row.display_title());
            positions.insert(rel.clone(), pages.len());
            pages.push(Page {
                path: root.join(&rel),
                rel,
                title,
                sources: vec![source],
                at,
            });
        }
    }
    Ok(Report { pages })
}

pub fn run(root: &Path, index: &Connection, args: &Args) -> Result<()> {
    let report = report(root, index, &args.session)?;
    if args.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for page in report.pages {
            println!(
                "{} ({}) [{}]",
                page.path.display(),
                page.title.as_deref().unwrap_or("[missing]"),
                page.sources.join(", ")
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_usage_is_empty_without_creating_a_database() -> Result<()> {
        let wiki = crate::usage::tests::Wiki::new()?;
        assert_eq!(
            serde_json::to_value(report(&wiki.root, &wiki.index, "s")?)?,
            json!({"pages": []})
        );
        assert!(!wiki.root.join("usage.sqlite").exists());
        Ok(())
    }

    #[test]
    fn newest_first_unique_paths_sources_missing_and_blocked_titles() -> Result<()> {
        let mut wiki = crate::usage::tests::Wiki::new()?;
        for name in ["a", "blocked", "removed"] {
            wiki.page(
                &format!("pages/knowledge/{name}.md"),
                "---\ntitle: Test\ncreated: 2020-01-01\n---\nbody",
            )?;
        }
        std::fs::remove_file(wiki.root.join("pages/knowledge/removed.md"))?;
        crate::index::refresh(&wiki.root, &mut wiki.index)?;
        wiki.index.execute(
            "update files set blocked='secret' where path='pages/knowledge/blocked.md'",
            [],
        )?;
        let usage = crate::usage::open(&wiki.root)?;
        for (session, name, source, at) in [
            ("s", "a", "remind", 10),
            ("s", "a", "remind", 50),
            ("s", "removed", "recall", 40),
            ("s", "a", "mod-agent", 30),
            ("s", "blocked", "context", 20),
            ("other", "blocked", "remind", 99),
        ] {
            usage.execute(
                "insert into injections values (?, ?, ?, ?)",
                rusqlite::params![session, format!("pages/knowledge/{name}.md"), source, at],
            )?;
        }
        assert_eq!(
            serde_json::to_value(report(&wiki.root, &wiki.index, "s")?)?,
            json!({"pages": [
                {"path": wiki.root.join("pages/knowledge/a.md"), "rel": "pages/knowledge/a.md", "title": "Test", "sources": ["remind", "mod-agent"], "at": 50},
                {"path": wiki.root.join("pages/knowledge/removed.md"), "rel": "pages/knowledge/removed.md", "title": null, "sources": ["recall"], "at": 40},
                {"path": wiki.root.join("pages/knowledge/blocked.md"), "rel": "pages/knowledge/blocked.md", "title": "[BLOCKED: secret]", "sources": ["context"], "at": 20}
            ]})
        );
        assert!(report(&wiki.root, &wiki.index, "absent")?.pages.is_empty());
        Ok(())
    }
}

use crate::index::{Row, COLUMNS, JOIN};
use anyhow::{bail, Result};
use chrono::{Local, TimeZone};
use clap::Args;
use regex::Regex;
use rusqlite::{params_from_iter, types::Value, Connection};
use std::{io::Write, path::Path, sync::LazyLock};

#[derive(Args, Default)]
pub struct Filters {
    #[arg(long = "type", value_parser = crate::pages::TYPES)]
    pub kind: Option<String>,
    #[arg(long, value_parser = ["open", "done"])]
    pub status: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub tag: Option<String>,
}
impl Filters {
    fn sql(&self) -> (String, Vec<Value>) {
        let mut sql = String::new();
        let mut params = vec![];
        for (column, value) in [
            ("type", &self.kind),
            ("status", &self.status),
            ("project", &self.project),
        ] {
            if let Some(value) = value {
                sql += &format!(" and fts.{column} = ?");
                params.push(Value::Text(value.clone()));
            }
        }
        if let Some(tag) = &self.tag {
            sql += " and (' ' || fts.tags || ' ') like ?";
            params.push(Value::Text(format!("% {tag} %")));
        }
        (sql, params)
    }
}

pub fn fts_query(terms: &[String]) -> String {
    static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());
    let mut parts = vec![];
    for term in terms {
        let words: Vec<_> = WORDS.find_iter(term).map(|m| m.as_str()).collect();
        let mut candidates = vec![];
        if words.len() > 1 {
            candidates.push(format!("\"{}\"", words.join(" ")));
        }
        candidates.extend(words.iter().map(|w| format!("\"{w}\"*")));
        for p in candidates {
            if !parts.contains(&p) {
                parts.push(p);
            }
        }
    }
    parts.join(" OR ")
}

pub fn updated(mtime: i64) -> String {
    Local
        .timestamp_opt(
            mtime.div_euclid(1_000_000_000),
            mtime.rem_euclid(1_000_000_000) as u32,
        )
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// The section of a page that matches the query best, for a hit's location line.
fn best_section(db: &Connection, path: &str, query: &str) -> Option<(String, i64, i64)> {
    db.query_row(
        "select sections.heading, sections.start_line, sections.end_line
         from sections_fts join sections on sections.id = sections_fts.rowid
         join files on files.id = sections.file_id
         where files.path = ? and sections_fts match ?
           and (select count(*) from sections others where others.file_id = sections.file_id) > 1
         order by bm25(sections_fts, 5, 1) limit 1",
        rusqlite::params![path, query],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .ok()
}

fn stale(mtime: i64) -> bool {
    let days = (Local::now().timestamp() - mtime.div_euclid(1_000_000_000)) / 86_400;
    days >= crate::index::STALE_DAYS
}

fn show(
    out: &mut impl Write,
    root: &Path,
    row: &Row,
    snippet: Option<String>,
    section: Option<(String, i64, i64)>,
) -> std::io::Result<()> {
    let path = root.join(&row.path).display().to_string();
    if !row.blocked.is_empty() {
        return writeln!(out, "{}", row.blocked_line(&path));
    }
    let mut meta = vec![if row.kind.is_empty() {
        "?".into()
    } else {
        row.kind.clone()
    }];
    if !row.status.is_empty() {
        meta.push(row.status.clone());
    }
    meta.push(format!(
        "updated {}{}",
        updated(row.mtime),
        if stale(row.mtime) { ", stale?" } else { "" }
    ));
    if !row.project.is_empty() {
        meta.push(format!("project={}", row.project));
    }
    if !row.tags.is_empty() {
        meta.push(format!(
            "tags={}",
            row.tags.split_whitespace().collect::<Vec<_>>().join(",")
        ));
    }
    if row.backlinks > 0 {
        meta.push(format!("linked by {}", row.backlinks));
    }
    writeln!(out, "{path}\n  {}  ({})", row.title, meta.join("; "))?;
    if let Some(s) = snippet.filter(|s| !s.is_empty()) {
        writeln!(
            out,
            "  {}",
            s.split_whitespace().collect::<Vec<_>>().join(" ")
        )?;
    }
    if let Some((heading, start, end)) = section {
        let heading = if heading.is_empty() {
            "(top)"
        } else {
            &heading
        };
        writeln!(out, "  § {heading} (lines {start}-{end})")?;
    }
    Ok(())
}

pub fn search(
    root: &Path,
    db: &Connection,
    terms: &[String],
    n: i64,
    filters: &Filters,
    no_semantic: bool,
) -> Result<()> {
    let query = fts_query(terms);
    if query.is_empty() {
        bail!("no search terms");
    }
    let (where_sql, params) = filters.sql();
    let mut args = vec![Value::Text(query.clone())];
    args.extend(params);
    args.push(Value::Integer(n));
    let mut stmt = db.prepare(&format!("select {COLUMNS}, snippet(fts, -1, '[', ']', '…', 16) {JOIN} where fts match ?{where_sql} order by bm25(fts, 0,0,0,0,10,5,3,1) limit ?"))?;
    let rows = stmt
        .query_map(params_from_iter(args), |r| {
            Ok((Row::read(r)?, r.get::<_, String>(9)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = std::io::stdout().lock();
    if rows.is_empty() {
        let total: i64 = db.query_row("select count(*) from files", [], |r| r.get(0))?;
        writeln!(out, "no pages match {query} ({total} pages in the wiki)")?;
        if !no_semantic && where_sql.is_empty() {
            let mut printed_heading = false;
            for hit in crate::semantic::search(&terms.join(" "), n) {
                // The daemon's snapshot can lag behind edits and deletions. Resolve
                // metadata here so stale or newly blocked text cannot bypass show().
                let Ok(row) = db.query_row(
                    &format!("select {COLUMNS} {JOIN} where files.path = ?"),
                    [format!("pages/{}.md", hit.path)],
                    Row::read,
                ) else {
                    continue;
                };
                if !printed_heading {
                    writeln!(out, "semantic matches (meaning, not keywords):")?;
                    printed_heading = true;
                }
                show(&mut out, root, &row, None, hit.section)?;
            }
        }
    }
    for (row, snippet) in rows {
        let section = if row.blocked.is_empty() {
            best_section(db, &row.path, &query)
        } else {
            None
        };
        show(&mut out, root, &row, Some(snippet), section)?;
    }
    Ok(())
}

pub fn list(root: &Path, db: &Connection, n: i64, filters: &Filters) -> Result<()> {
    let (where_sql, mut params) = filters.sql();
    params.push(Value::Integer(n));
    let mut stmt = db.prepare(&format!(
        "select {COLUMNS} {JOIN} where 1{where_sql} order by files.mtime_ns desc limit ?"
    ))?;
    let rows = stmt
        .query_map(params_from_iter(params), Row::read)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        println!("no pages");
    }
    for row in rows {
        show(&mut std::io::stdout().lock(), root, &row, None, None)?;
    }
    Ok(())
}

pub fn counts(items: impl IntoIterator<Item = String>) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = vec![];
    for item in items {
        if let Some((_, count)) = counts.iter_mut().find(|(key, _)| *key == item) {
            *count += 1;
        } else {
            counts.push((item, 1));
        }
    }
    counts.sort_by_key(|b| std::cmp::Reverse(b.1));
    counts
}

pub fn tag_counts(db: &Connection) -> Result<Vec<(String, usize)>> {
    let tags = db
        .prepare(&format!("select fts.tags {JOIN} where files.blocked = ''"))?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(counts(
        tags.iter()
            .flat_map(|s| s.split_whitespace().map(str::to_owned)),
    ))
}

pub fn tags(db: &Connection) -> Result<()> {
    let projects = db
        .prepare(&format!(
            "select fts.project {JOIN} where files.blocked = '' and fts.project != ''"
        ))?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (label, counts) in [("tags", tag_counts(db)?), ("projects", counts(projects))] {
        let text = counts
            .iter()
            .map(|(k, n)| format!("{k} ({n})"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("{label}: {}", if text.is_empty() { "none" } else { &text });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_phrases_prefixes_and_escaping() {
        assert_eq!(
            fts_query(&[
                "readonly database".into(),
                "database".into(),
                "\" OR *".into()
            ]),
            "\"readonly database\" OR \"readonly\"* OR \"database\"* OR \"OR\"*"
        );
        assert_eq!(fts_query(&["!*()".into()]), "");
    }

    #[test]
    fn render_with_and_without_section() {
        let row = Row {
            path: "pages/knowledge/foo.md".into(),
            kind: "knowledge".into(),
            status: String::new(),
            title: "Cache recovery".into(),
            project: "demo".into(),
            tags: "cache restart".into(),
            mtime: Local::now().timestamp_nanos_opt().unwrap(),
            blocked: String::new(),
            backlinks: 2,
        };
        let expected = format!(
            "/wiki/pages/knowledge/foo.md\n  Cache recovery  (knowledge; updated {}; project=demo; tags=cache,restart; linked by 2)\n",
            updated(row.mtime)
        );
        for section in [
            None,
            Some(("What kills it".into(), 27, 36)),
            Some((String::new(), 1, 3)),
        ] {
            let mut out = Vec::new();
            show(&mut out, Path::new("/wiki"), &row, None, section.clone()).unwrap();
            let location = match section {
                Some((heading, start, end)) => format!(
                    "  § {} (lines {start}-{end})\n",
                    if heading.is_empty() {
                        "(top)"
                    } else {
                        &heading
                    }
                ),
                None => String::new(),
            };
            assert_eq!(
                String::from_utf8(out).unwrap(),
                format!("{expected}{location}")
            );
        }
        let mut out = Vec::new();
        let blocked = Row {
            blocked: "secret".into(),
            ..row
        };
        show(
            &mut out,
            Path::new("/wiki"),
            &blocked,
            None,
            Some(("hidden".into(), 1, 3)),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "/wiki/pages/knowledge/foo.md: [BLOCKED: secret; fix the page]\n"
        );
    }
}

use crate::index::{Row, COLUMNS, JOIN};
use anyhow::{bail, Result};
use chrono::{Local, TimeZone};
use clap::Args;
use regex::Regex;
use rusqlite::{params_from_iter, types::Value, Connection};
use std::{path::Path, sync::LazyLock};

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

fn show(root: &Path, row: &Row, snippet: Option<String>) {
    let path = root.join(&row.path).display().to_string();
    if !row.blocked.is_empty() {
        println!("{}", row.blocked_line(&path));
        return;
    }
    let mut meta = vec![if row.kind.is_empty() {
        "?".into()
    } else {
        row.kind.clone()
    }];
    if !row.status.is_empty() {
        meta.push(row.status.clone());
    }
    meta.push(format!("updated {}", updated(row.mtime)));
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
    println!("{path}\n  {}  ({})", row.title, meta.join("; "));
    if let Some(s) = snippet.filter(|s| !s.is_empty()) {
        println!("  {}", s.split_whitespace().collect::<Vec<_>>().join(" "));
    }
}

pub fn search(
    root: &Path,
    db: &Connection,
    terms: &[String],
    n: i64,
    filters: &Filters,
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
    if rows.is_empty() {
        let total: i64 = db.query_row("select count(*) from files", [], |r| r.get(0))?;
        println!("no pages match {query} ({total} pages in the wiki)");
    }
    for (row, snippet) in rows {
        show(root, &row, Some(snippet));
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
        show(root, &row, None);
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
}

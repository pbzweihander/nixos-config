use crate::index::{COLUMNS, JOIN, Row};
use crate::semantic::DenseStatus;
use anyhow::{Result, bail};
use chrono::{Local, TimeZone};
use clap::Args;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};
use serde_json::json;
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
    meaning: bool,
) -> std::io::Result<()> {
    let path = format!(
        "{}{}",
        root.join(&row.path).display(),
        if meaning { " (meaning)" } else { "" }
    );
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

pub fn bm25(db: &Connection, query: &str, n: i64, filters: &Filters) -> Result<Vec<(Row, String)>> {
    if query.is_empty() {
        return Ok(vec![]);
    }
    let (where_sql, params) = filters.sql();
    let mut args = vec![Value::Text(query.into())];
    args.extend(params);
    args.push(Value::Integer(n));
    let mut stmt = db.prepare(&format!("select {COLUMNS}, snippet(fts, -1, '[', ']', '…', 16) {JOIN} where fts match ?{where_sql} order by bm25(fts, 0,0,0,0,10,5,3,1) limit ?"))?;
    let rows = stmt
        .query_map(params_from_iter(args), |r| {
            Ok((Row::read(r)?, r.get::<_, String>(9)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub const CANDIDATES: i64 = 40;

pub struct SearchHit {
    pub row: Row,
    snippet: String,
    section: Option<(String, i64, i64)>,
    pub bm25_rank: Option<usize>,
    pub dense_rank: Option<usize>,
}

impl SearchHit {
    fn score(&self) -> f64 {
        let (bm25, bm25_denominator) = self
            .bm25_rank
            .map_or((0.0, 1.0), |rank| (2.0, 60.0 + rank as f64));
        let (dense, dense_denominator) = self
            .dense_rank
            .map_or((0.0, 1.0), |rank| (1.0, 60.0 + rank as f64));
        let (weight, scale) = if self.row.kind == "history" {
            (9.0, 10.0)
        } else {
            (1.0, 1.0)
        };
        // Divide once so equal rational scores tie, without rounding each term
        // separately (e.g. ranks 4/36 and 6/28 both score 1/24).
        weight * (bm25 * dense_denominator + dense * bm25_denominator)
            / (scale * bm25_denominator * dense_denominator)
    }

    fn via(&self) -> &'static str {
        match (self.bm25_rank, self.dense_rank) {
            (Some(_), Some(_)) => "both",
            (Some(_), None) => "bm25",
            _ => "dense",
        }
    }
}

fn sort_hits(hits: &mut [SearchHit]) {
    hits.sort_by(|a, b| {
        b.score()
            .total_cmp(&a.score())
            .then_with(|| {
                a.bm25_rank
                    .unwrap_or(usize::MAX)
                    .cmp(&b.bm25_rank.unwrap_or(usize::MAX))
            })
            .then_with(|| a.row.path.cmp(&b.row.path))
    });
}

fn fuse(
    db: &Connection,
    bm25: Vec<(Row, String)>,
    dense: Vec<crate::semantic::Hit>,
    filters: &Filters,
) -> Result<Vec<SearchHit>> {
    let mut hits: Vec<_> = bm25
        .into_iter()
        .enumerate()
        .map(|(i, (row, snippet))| SearchHit {
            row,
            snippet,
            section: None,
            bm25_rank: Some(i + 1),
            dense_rank: None,
        })
        .collect();
    let (where_sql, params) = filters.sql();
    let mut stmt = db.prepare(&format!(
        "select {COLUMNS} {JOIN} where files.path = ?{where_sql}"
    ))?;
    // Keep the daemon's original ranks after filtering its top 40 candidates.
    for (i, hit) in dense.into_iter().take(CANDIDATES as usize).enumerate() {
        let path = format!("pages/{}.md", hit.path);
        if let Some(existing) = hits.iter_mut().find(|hit| hit.row.path == path) {
            existing.dense_rank.get_or_insert(i + 1);
            continue;
        }
        let mut args = vec![Value::Text(path)];
        args.extend(params.iter().cloned());
        // Resolve current metadata, including blocked flags, from the local index.
        // The daemon's snapshot can lag behind edits and deletions.
        if let Some(row) = stmt
            .query_row(params_from_iter(args), Row::read)
            .optional()?
        {
            hits.push(SearchHit {
                row,
                snippet: String::new(),
                section: hit.section,
                bm25_rank: None,
                dense_rank: Some(i + 1),
            });
        }
    }
    sort_hits(&mut hits);
    Ok(hits)
}

pub fn ranked(
    db: &Connection,
    query: &str,
    terms: &str,
    n: i64,
    filters: &Filters,
    no_semantic: bool,
) -> Result<(Vec<SearchHit>, DenseStatus)> {
    if n == 0 {
        return Ok((vec![], DenseStatus::Disabled));
    }
    // Preserve large and unlimited SQLite limits when dense is unavailable.
    let mut rows = bm25(
        db,
        query,
        if n < 0 { n } else { n.max(CANDIDATES) },
        filters,
    )?;
    let (dense, status) = if no_semantic {
        (vec![], DenseStatus::Disabled)
    } else {
        crate::semantic::dense_with_status(terms, CANDIDATES)
    };
    if !dense.is_empty() {
        rows.truncate(CANDIDATES as usize);
    }
    Ok((fuse(db, rows, dense, filters)?, status))
}

pub fn search(
    root: &Path,
    db: &Connection,
    terms: &[String],
    n: i64,
    filters: &Filters,
    no_semantic: bool,
    json: bool,
) -> Result<()> {
    let query = fts_query(terms);
    if query.is_empty() {
        bail!("no search terms");
    }
    let (mut rows, dense) = ranked(db, &query, &terms.join(" "), n, filters, no_semantic)?;
    if !json && let DenseStatus::Unavailable(reason) = &dense {
        eprintln!("note: dense retrieval unavailable ({reason}); results are BM25 only");
    }
    if n >= 0 {
        rows.truncate(n as usize);
    }
    let json_root = if json {
        Some(root.canonicalize()?)
    } else {
        None
    };
    let mut hits = vec![];
    let mut out = std::io::stdout().lock();
    if rows.is_empty() && !json {
        let total: i64 = db.query_row("select count(*) from files", [], |r| r.get(0))?;
        writeln!(out, "no pages match {query} ({total} pages in the wiki)")?;
    }
    for hit in rows {
        let via = hit.via();
        let section = if !hit.row.blocked.is_empty() {
            None
        } else if hit.bm25_rank.is_some() {
            best_section(db, &hit.row.path, &query)
        } else {
            hit.section
        };
        if let Some(root) = &json_root {
            hits.push(json_hit(root, &hit.row, &hit.snippet, section, via));
        } else {
            show(
                &mut out,
                root,
                &hit.row,
                Some(hit.snippet),
                section,
                via == "dense",
            )?;
        }
    }
    if json {
        let mut report = json!({"hits": hits, "dense": dense.name()});
        if let DenseStatus::Unavailable(reason) = dense {
            report["dense_error"] = reason.into();
        }
        writeln!(out, "{report}")?;
    }
    Ok(())
}

fn json_hit(
    root: &Path,
    row: &Row,
    snippet: &str,
    section: Option<(String, i64, i64)>,
    via: &str,
) -> serde_json::Value {
    let blocked = !row.blocked.is_empty();
    let section = if blocked {
        None
    } else {
        section.map(crate::semantic::Section::from)
    };
    json!({
        "path": root.join(&row.path),
        "rel": row.path,
        "title": row.display_title(),
        "type": row.kind,
        "status": row.status,
        "project": row.project,
        "tags": row.tags.split_whitespace().collect::<Vec<_>>(),
        "updated": updated(row.mtime),
        "stale": stale(row.mtime),
        "backlinks": row.backlinks,
        "snippet": if blocked { "" } else { snippet },
        "section": section,
        "semantic": via == "dense",
        "via": via,
    })
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
        show(&mut std::io::stdout().lock(), root, &row, None, None, false)?;
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

    fn hit(path: &str, bm25_rank: Option<usize>, dense_rank: Option<usize>) -> SearchHit {
        SearchHit {
            row: Row {
                path: format!("pages/{path}.md"),
                kind: path.split('/').next().unwrap().into(),
                status: String::new(),
                title: path.into(),
                project: String::new(),
                tags: String::new(),
                mtime: 0,
                blocked: String::new(),
                backlinks: 0,
            },
            snippet: String::new(),
            section: None,
            bm25_rank,
            dense_rank,
        }
    }

    #[test]
    fn weighted_reciprocal_ranks_and_history_penalty() {
        let both = hit("knowledge/both", Some(2), Some(5));
        assert!((both.score() - (2.0 / 62.0 + 1.0 / 65.0)).abs() < 1e-15);
        assert_eq!(both.via(), "both");
        let bm25 = hit("knowledge/bm25", Some(1), None);
        assert_eq!(bm25.score(), 2.0 / 61.0);
        assert_eq!(bm25.via(), "bm25");
        let dense = hit("knowledge/dense", None, Some(40));
        assert_eq!(dense.score(), 1.0 / 100.0);
        assert_eq!(dense.via(), "dense");
        for (bm25, dense) in [(Some(1), None), (None, Some(1)), (Some(1), Some(1))] {
            let history = hit("history/past", bm25, dense);
            assert!(
                (history.score() - hit("knowledge/current", bm25, dense).score() * 0.9).abs()
                    < 1e-15
            );
            let mut hits = vec![
                history,
                hit("knowledge/current", bm25.map(|_| 2), dense.map(|_| 2)),
            ];
            sort_hits(&mut hits);
            assert_eq!(hits[0].row.kind, "knowledge");
        }
    }

    #[test]
    fn fusion_orders_by_score_then_bm25_rank_then_path() {
        let mut hits = vec![
            hit("knowledge/a", Some(6), Some(28)),
            hit("knowledge/z", Some(4), Some(36)),
            hit("knowledge/dense-z", None, Some(1)),
            hit("knowledge/dense-a", None, Some(1)),
            hit("knowledge/first", Some(1), Some(1)),
        ];
        assert_eq!(hits[0].score(), hits[1].score());
        sort_hits(&mut hits);
        assert_eq!(
            hits.iter()
                .map(|h| h.row.title.as_str())
                .collect::<Vec<_>>(),
            [
                "knowledge/first",
                "knowledge/z",
                "knowledge/a",
                "knowledge/dense-a",
                "knowledge/dense-z"
            ]
        );
    }

    #[test]
    fn json_metadata_sections_and_blocked_redaction() {
        let mut row = Row {
            path: "pages/knowledge/foo.md".into(),
            kind: "knowledge".into(),
            status: "".into(),
            title: "Cache notes".into(),
            project: "demo".into(),
            tags: "cache restart".into(),
            mtime: 0,
            blocked: "".into(),
            backlinks: 2,
        };
        let section = Some(("Details".into(), 1, 9));
        assert_eq!(
            json_hit(
                Path::new("/wiki"),
                &row,
                "[cache] details",
                section.clone(),
                "bm25"
            ),
            json!({
                "path": "/wiki/pages/knowledge/foo.md", "rel": "pages/knowledge/foo.md",
                "title": "Cache notes", "type": "knowledge", "status": "", "project": "demo",
                "tags": ["cache", "restart"], "updated": updated(0), "stale": true, "backlinks": 2,
                "snippet": "[cache] details", "section": {"heading": "Details", "start": 1, "end": 9}, "semantic": false, "via": "bm25"
            })
        );
        row.mtime = Local::now().timestamp_nanos_opt().unwrap();
        let value = json_hit(Path::new("/wiki"), &row, "", None, "dense");
        assert_eq!(value["semantic"], true);
        assert_eq!(value["stale"], false);
        assert!(value["section"].is_null());
        row.blocked = "secret".into();
        let value = json_hit(Path::new("/wiki"), &row, "hidden snippet", section, "dense");
        assert_eq!(value["title"], "[BLOCKED: secret]");
        assert_eq!(value["snippet"], "");
        assert!(value["section"].is_null());
        assert!(!value.to_string().contains("Cache notes"));
        assert!(!value.to_string().contains("Details"));
    }

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
            show(
                &mut out,
                Path::new("/wiki"),
                &row,
                None,
                section.clone(),
                false,
            )
            .unwrap();
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
            false,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "/wiki/pages/knowledge/foo.md: [BLOCKED: secret; fix the page]\n"
        );
    }
}

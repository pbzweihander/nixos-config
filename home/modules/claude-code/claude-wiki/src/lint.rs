use crate::{index, pages, scan, semantic, sync, usage};
use anyhow::Result;
use chrono::Utc;
use clap::Args as ClapArgs;
use regex::Regex;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::LazyLock,
};

#[derive(ClapArgs)]
pub struct Args {
    #[arg(long)]
    pub all: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long, default_value_t = 0.88, value_parser = cosine)]
    pub min: f64,
}

fn cosine(value: &str) -> std::result::Result<f64, String> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (-1.0..=1.0).contains(v))
        .ok_or_else(|| "min must be a finite cosine between -1 and 1".into())
}

/// A slug date marks a snapshot only when it is the page's own date. A date far from
/// `created`, such as a release cutoff (`minio-before-2024-09-13-...`), is a fact about
/// the topic, not a snapshot. Without a valid `created`, any valid slug date counts.
fn snapshot_slug(date: &Regex, stem: &str, created: &str) -> bool {
    let parse = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    let created = parse(created.trim());
    date.find_iter(stem)
        .filter_map(|m| parse(m.as_str()))
        .any(|d| created.is_none_or(|c| (d - c).num_days().abs() <= 7))
}

fn worklist(root: &Path, db: &Connection, now: i64) -> Result<BTreeMap<String, Vec<Value>>> {
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap());
    static LOG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}|\bRound\s+\d+\b|^Update\b").unwrap());
    let mut result: BTreeMap<String, Vec<Value>> = [
        "duplicates",
        "snapshots",
        "log-growth",
        "stale-followups",
        "orphans",
        "frontmatter",
        "broken-links",
        "missing-queries",
        "singleton-tags",
    ]
    .into_iter()
    .map(|s| (s.into(), vec![]))
    .collect();
    let unread = if root.join("usage.sqlite").is_file() {
        let usage = usage::open(root)?;
        usage::scanned(&usage)?
            .then(|| usage::read_pages(&usage, i64::MIN))
            .transpose()?
    } else {
        None
    };
    if unread.is_some() {
        result.insert("unread".into(), vec![]);
    }
    let mut tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let rows = db
        .prepare(&format!(
            "select {} {} order by files.path",
            index::COLUMNS,
            index::JOIN
        ))?
        .query_map([], index::Row::read)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for row in rows {
        let text = pages::read(&root.join(&row.path))?;
        let page = pages::parse(&text, &row.path);
        let mut add = |category: &str, detail: String| {
            result
                .get_mut(category)
                .unwrap()
                .push(json!({"path": row.path, "detail": detail}));
        };
        for problem in &page.problems {
            add("frontmatter", scan::redact(problem));
        }
        for finding in sync::broken_links(root, &page) {
            add("broken-links", finding);
        }
        if page.kind == "knowledge"
            && snapshot_slug(
                &DATE,
                Path::new(&row.path)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref(),
                &page.created,
            )
        {
            add("snapshots", "dated knowledge slug".into());
        }
        if page.kind == "knowledge" {
            if row.backlinks == 0 && page.links.is_empty() {
                add("orphans", "no backlinks or outgoing links".into());
            }
            if page.queries.is_empty() {
                add("missing-queries", "no queries".into());
            }
        }
        if matches!(page.kind.as_str(), "knowledge" | "followups") {
            let count = pages::sections(&text)
                .iter()
                .filter(|s| LOG.is_match(s.heading.rsplit(" > ").next().unwrap_or_default()))
                .count();
            if count >= 3 {
                add(
                    "log-growth",
                    format!("{count} dated, Round or Update headings"),
                );
            }
        }
        if page.kind == "followups"
            && page.status == "open"
            && now - row.mtime / 1_000_000_000 > 14 * 86400
        {
            let age = (now - row.mtime / 1_000_000_000) / 86400;
            result
                .get_mut("stale-followups")
                .unwrap()
                .push(json!({"path": row.path, "age_days": age, "mtime": row.mtime}));
        }
        if unread
            .as_ref()
            .is_some_and(|read| !read.contains(&row.path))
            && usage::older_than(&page.created, row.mtime, now, 14)
        {
            result
                .get_mut("unread")
                .unwrap()
                .push(json!({"path": row.path, "detail": "older than 14 days, never read"}));
        }
        if row.blocked.is_empty() {
            for tag in page.tags.split_whitespace().collect::<HashSet<_>>() {
                tags.entry(tag.into()).or_default().push(row.path.clone());
            }
        }
    }
    for (tag, paths) in tags {
        if paths.len() == 1 {
            result
                .get_mut("singleton-tags")
                .unwrap()
                .push(json!({"tag": tag, "path": paths[0]}));
        }
    }
    result.get_mut("stale-followups").unwrap().sort_by(|a, b| {
        a["mtime"]
            .as_i64()
            .cmp(&b["mtime"].as_i64())
            .then(a["path"].as_str().cmp(&b["path"].as_str()))
    });
    Ok(result)
}

pub fn run(root: &Path, db: &Connection, args: &Args) -> Result<()> {
    let mut result = worklist(root, db, Utc::now().timestamp())?;
    let eligible: HashSet<String> = db.prepare("select fts.path from fts join files on files.id=fts.rowid where files.blocked='' and (fts.type='knowledge' or (fts.type='followups' and fts.status='open'))")?
        .query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let available = if let Some(pairs) = semantic::pairs(args.min) {
        let mut pairs: Vec<_> = pairs
            .into_iter()
            .filter_map(|p| {
                let (a, b) = (pages::link_path(&p.a)?, pages::link_path(&p.b)?);
                (a != b
                    && eligible.contains(&a)
                    && eligible.contains(&b)
                    && p.score.is_finite()
                    && p.score >= args.min)
                    .then_some((a, b, p.score))
            })
            .collect();
        pairs.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
        result.insert(
            "duplicates".into(),
            pairs
                .into_iter()
                .map(|(a, b, score)| json!({"a": a, "b": b, "score": score}))
                .collect(),
        );
        true
    } else {
        false
    };
    if args.json {
        if !available {
            eprintln!("note: duplicates: daemon unavailable");
        }
        println!("{}", serde_json::to_string(&result)?);
    } else {
        for (category, items) in result {
            println!(
                "{category} ({}){}",
                items.len(),
                if category == "duplicates" && !available {
                    ": daemon unavailable"
                } else {
                    ""
                }
            );
            let limit = if args.all { items.len() } else { 20 };
            for item in items.iter().take(limit) {
                if let Some(score) = item["score"].as_f64() {
                    println!(
                        "  {} <-> {} ({score:.2})",
                        item["a"].as_str().unwrap(),
                        item["b"].as_str().unwrap()
                    );
                } else {
                    let detail = if let Some(age) = item["age_days"].as_i64() {
                        format!("{age} days old")
                    } else {
                        item["detail"]
                            .as_str()
                            .or_else(|| item["tag"].as_str())
                            .unwrap_or_default()
                            .into()
                    };
                    println!("  {}: {detail}", item["path"].as_str().unwrap());
                }
            }
            if items.len() > limit {
                println!("  … and {} more", items.len() - limit);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_slug_needs_a_date_near_created() {
        let date = Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
        assert!(snapshot_slug(&date, "review-2026-09-19", "2026-09-19"));
        assert!(snapshot_slug(&date, "review-2026-09-19", "2026-09-24"));
        assert!(!snapshot_slug(
            &date,
            "minio-before-2024-09-13-ignores",
            "2026-09-28"
        ));
        assert!(snapshot_slug(&date, "review-2026-09-19", ""));
        assert!(!snapshot_slug(&date, "review-2026-13-45", ""));
        assert!(!snapshot_slug(&date, "plain-topic", "2026-09-19"));
    }

    #[test]
    fn categories_are_complete_and_deterministic() -> Result<()> {
        let mut wiki = usage::tests::Wiki::new()?;
        wiki.page("pages/knowledge/2020-01-01-review.md", "---\ntitle: Review\ncreated: 2020-01-01\ntags: [unique, shared]\n---\n## Round 1\n## Update findings\n## 2020-02-03\n```\n## Update ignored\n```\n")?;
        wiki.page("pages/knowledge/linked.md", "---\ntitle: Linked\ncreated: 2020-01-01\ntags: [shared]\nqueries: [why]\n---\n[[knowledge/missing]]")?;
        wiki.page(
            "pages/followups/old.md",
            "---\ntitle: Old\ncreated: 2020-01-01\nstatus: open\n---\ntext",
        )?;
        wiki.page(
            "pages/followups/done.md",
            "---\ntitle: Done\ncreated: 2020-01-01\nstatus: done\n---\ntext",
        )?;
        wiki.page("pages/history/invalid.md", "no frontmatter")?;
        let now = Utc::now().timestamp();
        wiki.index.execute(
            "update files set mtime_ns=? where path like 'pages/followups/%'",
            [(now - 20 * 86400) * 1_000_000_000],
        )?;
        let initial = worklist(&wiki.root, &wiki.index, now)?;
        assert!(!initial.contains_key("unread"));
        for category in [
            "snapshots",
            "log-growth",
            "stale-followups",
            "orphans",
            "frontmatter",
            "broken-links",
            "missing-queries",
            "singleton-tags",
        ] {
            assert_eq!(
                initial[category].len(),
                1,
                "{category}: {:?}",
                initial[category]
            );
        }
        assert_eq!(initial, worklist(&wiki.root, &wiki.index, now)?);
        let usage = usage::open(&wiki.root)?;
        usage.execute_batch("insert into state values ('scanned', 1), ('scan_format', 2); insert into tools values ('r','s',1); insert into accesses values ('r','pages/knowledge/linked.md','read');")?;
        let after = worklist(&wiki.root, &wiki.index, now)?;
        assert_eq!(after["unread"].len(), 3);
        assert_eq!(after["stale-followups"][0]["age_days"], 20);
        Ok(())
    }
}

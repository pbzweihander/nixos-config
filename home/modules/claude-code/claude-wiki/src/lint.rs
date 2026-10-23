use crate::{index, pages, scan, semantic, sync, usage};
use anyhow::Result;
use chrono::Utc;
use clap::Args as ClapArgs;
use regex::Regex;
use rusqlite::Connection;
use serde::Serialize;
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

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct Report {
    #[serde(rename = "broken-links")]
    broken_links: Vec<Finding>,
    duplicates: Vec<Duplicate>,
    frontmatter: Vec<Finding>,
    #[serde(rename = "log-growth")]
    log_growth: Vec<Finding>,
    #[serde(rename = "missing-queries")]
    missing_queries: Vec<Finding>,
    orphans: Vec<Finding>,
    #[serde(rename = "singleton-tags")]
    singleton_tags: Vec<SingletonTag>,
    snapshots: Vec<Finding>,
    #[serde(rename = "stale-followups")]
    stale_followups: Vec<StaleFollowup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unread: Option<Vec<Finding>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct Finding {
    detail: String,
    path: String,
    usage: usage::PageUsage,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct Duplicate {
    a: String,
    b: String,
    score: f64,
    usage_a: usage::PageUsage,
    usage_b: usage::PageUsage,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct SingletonTag {
    path: String,
    tag: String,
    usage: usage::PageUsage,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
struct StaleFollowup {
    age_days: i64,
    mtime: i64,
    path: String,
    usage: usage::PageUsage,
}

trait Item {
    fn fill_usage(&mut self, usage: &BTreeMap<String, usage::PageUsage>);
    fn importance(&self) -> f64;
    fn line(&self) -> String;
}

impl Item for Finding {
    fn fill_usage(&mut self, usage: &BTreeMap<String, usage::PageUsage>) {
        self.usage = usage.get(&self.path).cloned().unwrap_or_default();
    }

    fn importance(&self) -> f64 {
        self.usage.importance
    }

    fn line(&self) -> String {
        format!(
            "{}: {}{}",
            self.path,
            self.detail,
            reads_marker(&self.usage)
        )
    }
}

impl Item for Duplicate {
    fn fill_usage(&mut self, usage: &BTreeMap<String, usage::PageUsage>) {
        self.usage_a = usage.get(&self.a).cloned().unwrap_or_default();
        self.usage_b = usage.get(&self.b).cloned().unwrap_or_default();
    }

    fn importance(&self) -> f64 {
        self.usage_a.importance.max(self.usage_b.importance)
    }

    fn line(&self) -> String {
        format!(
            "{}{} <-> {}{} ({:.2})",
            self.a,
            reads_marker(&self.usage_a),
            self.b,
            reads_marker(&self.usage_b),
            self.score
        )
    }
}

impl Item for SingletonTag {
    fn fill_usage(&mut self, usage: &BTreeMap<String, usage::PageUsage>) {
        self.usage = usage.get(&self.path).cloned().unwrap_or_default();
    }

    fn importance(&self) -> f64 {
        self.usage.importance
    }

    fn line(&self) -> String {
        format!("{}: {}{}", self.path, self.tag, reads_marker(&self.usage))
    }
}

impl Item for StaleFollowup {
    fn fill_usage(&mut self, usage: &BTreeMap<String, usage::PageUsage>) {
        self.usage = usage.get(&self.path).cloned().unwrap_or_default();
    }

    fn importance(&self) -> f64 {
        self.usage.importance
    }

    fn line(&self) -> String {
        format!(
            "{}: {} days old{}",
            self.path,
            self.age_days,
            reads_marker(&self.usage)
        )
    }
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

fn worklist(root: &Path, db: &Connection, now: i64) -> Result<Report> {
    static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap());
    static LOG: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}|\bRound\s+\d+\b|^Update\b").unwrap());
    let unread = if root.join("usage.sqlite").is_file() {
        let usage = usage::open(root)?;
        usage::scanned(&usage)?
            .then(|| usage::read_pages(&usage, i64::MIN))
            .transpose()?
    } else {
        None
    };
    let mut result = Report {
        unread: unread.as_ref().map(|_| vec![]),
        ..Report::default()
    };
    let mut unread_ages = BTreeMap::new();
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
        let add = |items: &mut Vec<Finding>, detail: String| {
            items.push(Finding {
                detail,
                path: row.path.clone(),
                usage: usage::PageUsage::default(),
            });
        };
        for problem in &page.problems {
            add(&mut result.frontmatter, scan::redact(problem));
        }
        for finding in sync::broken_links(root, &page) {
            add(&mut result.broken_links, finding);
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
            add(&mut result.snapshots, "dated knowledge slug".into());
        }
        if page.kind == "knowledge" {
            if row.backlinks == 0 && page.links.is_empty() {
                add(&mut result.orphans, "no backlinks or outgoing links".into());
            }
            if page.queries.is_empty() {
                add(&mut result.missing_queries, "no queries".into());
            }
        }
        if matches!(page.kind.as_str(), "knowledge" | "followups") {
            let count = pages::sections(&text)
                .iter()
                .filter(|s| LOG.is_match(s.heading.rsplit(" > ").next().unwrap_or_default()))
                .count();
            if count >= 3 {
                add(
                    &mut result.log_growth,
                    format!("{count} dated, Round or Update headings"),
                );
            }
        }
        if page.kind == "followups"
            && page.status == "open"
            && now - row.mtime / 1_000_000_000 > 14 * 86400
        {
            let age = (now - row.mtime / 1_000_000_000) / 86400;
            result.stale_followups.push(StaleFollowup {
                age_days: age,
                mtime: row.mtime,
                path: row.path.clone(),
                usage: usage::PageUsage::default(),
            });
        }
        if unread
            .as_ref()
            .is_some_and(|read| !read.contains(&row.path))
            && usage::older_than(&page.created, row.mtime, now, 14)
        {
            unread_ages.insert(
                row.path.clone(),
                usage::age_seconds(&page.created, row.mtime, now),
            );
            if let Some(items) = &mut result.unread {
                add(items, "older than 14 days, never read".into());
            }
        }
        if row.blocked.is_empty() {
            for tag in page.tags.split_whitespace().collect::<HashSet<_>>() {
                tags.entry(tag.into()).or_default().push(row.path.clone());
            }
        }
    }
    for (tag, paths) in tags {
        if paths.len() == 1 {
            result.singleton_tags.push(SingletonTag {
                path: paths[0].clone(),
                tag,
                usage: usage::PageUsage::default(),
            });
        }
    }
    result
        .stale_followups
        .sort_by(|a, b| a.mtime.cmp(&b.mtime).then(a.path.cmp(&b.path)));
    if let Some(items) = &mut result.unread {
        items.sort_by(|a, b| {
            unread_ages[&b.path]
                .cmp(&unread_ages[&a.path])
                .then(a.path.cmp(&b.path))
        });
    }
    Ok(result)
}

fn prioritize(result: &mut Report, usage: &BTreeMap<String, usage::PageUsage>) {
    fn fill(items: &mut [impl Item], usage: &BTreeMap<String, usage::PageUsage>, sort: bool) {
        for item in items.iter_mut() {
            item.fill_usage(usage);
        }
        if sort {
            // Stable sorting preserves the original path/tag/score order on ties.
            items.sort_by(|a, b| b.importance().total_cmp(&a.importance()));
        }
    }
    for items in [
        &mut result.broken_links,
        &mut result.frontmatter,
        &mut result.log_growth,
        &mut result.missing_queries,
        &mut result.orphans,
        &mut result.snapshots,
    ] {
        fill(items, usage, true);
    }
    fill(&mut result.duplicates, usage, true);
    fill(&mut result.singleton_tags, usage, true);
    fill(&mut result.stale_followups, usage, false);
    if let Some(items) = &mut result.unread {
        fill(items, usage, false);
    }
}

fn reads_marker(usage: &usage::PageUsage) -> String {
    if usage.reads > 0 {
        format!(" · {} reads", usage.reads)
    } else {
        String::new()
    }
}

fn print_category(category: &str, items: &[impl Item], all: bool) {
    println!("{category} ({})", items.len());
    print_items(items, all);
}

fn print_items(items: &[impl Item], all: bool) {
    let limit = if all { items.len() } else { 20 };
    for item in items.iter().take(limit) {
        println!("  {}", item.line());
    }
    if items.len() > limit {
        println!("  … and {} more", items.len() - limit);
    }
}

pub fn run(root: &Path, db: &Connection, args: &Args) -> Result<()> {
    let usage = usage::refresh(root)?;
    let now = Utc::now().timestamp();
    let mut result = worklist(root, db, now)?;
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
        result.duplicates = pairs
            .into_iter()
            .map(|(a, b, score)| Duplicate {
                a,
                b,
                score,
                ..Duplicate::default()
            })
            .collect();
        true
    } else {
        false
    };
    prioritize(&mut result, &usage::per_page(&usage, now)?);
    if args.json {
        if !available {
            eprintln!("note: duplicates: daemon unavailable");
        }
        println!("{}", serde_json::to_string(&result)?);
    } else {
        print_category("broken-links", &result.broken_links, args.all);
        println!(
            "duplicates ({}){}",
            result.duplicates.len(),
            if available {
                ""
            } else {
                ": daemon unavailable"
            }
        );
        print_items(&result.duplicates, args.all);
        print_category("frontmatter", &result.frontmatter, args.all);
        print_category("log-growth", &result.log_growth, args.all);
        print_category("missing-queries", &result.missing_queries, args.all);
        print_category("orphans", &result.orphans, args.all);
        print_category("singleton-tags", &result.singleton_tags, args.all);
        print_category("snapshots", &result.snapshots, args.all);
        print_category("stale-followups", &result.stale_followups, args.all);
        if let Some(items) = &result.unread {
            print_category("unread", items, args.all);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_is_oldest_first_using_created_or_mtime() -> Result<()> {
        let mut wiki = usage::tests::Wiki::new()?;
        wiki.page(
            "pages/knowledge/a.md",
            "---\ntitle: Newer\ncreated: 2020-03-01\n---\ntext",
        )?;
        wiki.page(
            "pages/knowledge/z.md",
            "---\ntitle: Older\ncreated: 2020-01-01\n---\ntext",
        )?;
        wiki.page(
            "pages/knowledge/b.md",
            "---\ntitle: Missing date\n---\ntext",
        )?;
        let now = Utc::now().timestamp();
        wiki.index.execute(
            "update files set mtime_ns=0 where path='pages/knowledge/b.md'",
            [],
        )?;
        let db = usage::open(&wiki.root)?;
        db.execute_batch("insert into state values ('scanned', 1), ('scan_format', 2);")?;
        let result = worklist(&wiki.root, &wiki.index, now)?;
        assert_eq!(
            result
                .unread
                .as_ref()
                .unwrap()
                .iter()
                .map(|i| i.path.as_str())
                .collect::<Vec<_>>(),
            [
                "pages/knowledge/b.md",
                "pages/knowledge/z.md",
                "pages/knowledge/a.md"
            ]
        );
        Ok(())
    }

    #[test]
    fn usage_fields_and_stable_ordering_include_pairs_and_age_exceptions() {
        let findings = |paths: &[&str]| {
            paths
                .iter()
                .map(|path| Finding {
                    path: (*path).into(),
                    ..Finding::default()
                })
                .collect()
        };
        let original = Report {
            duplicates: [("a", "b", 0.99), ("c", "d", 0.95), ("e", "d", 0.90)]
                .into_iter()
                .map(|(a, b, score)| Duplicate {
                    a: a.into(),
                    b: b.into(),
                    score,
                    ..Duplicate::default()
                })
                .collect(),
            orphans: findings(&["a", "b", "d", "e"]),
            stale_followups: [("a", 30), ("d", 20)]
                .into_iter()
                .map(|(path, age_days)| StaleFollowup {
                    age_days,
                    path: path.into(),
                    ..StaleFollowup::default()
                })
                .collect(),
            unread: Some(findings(&["a", "d"])),
            ..Report::default()
        };
        let mut empty = original.clone();
        prioritize(&mut empty, &BTreeMap::new());
        assert_eq!(empty, original);
        let zero = usage::PageUsage {
            importance: 0.0,
            injected_unread: 0,
            last_read: None,
            read_sessions: 0,
            reads: 0,
        };
        for item in &empty.duplicates {
            assert_eq!(item.usage_a, zero);
            assert_eq!(item.usage_b, zero);
        }
        for items in [&empty.orphans, empty.unread.as_ref().unwrap()] {
            for item in items {
                assert_eq!(item.usage, zero);
            }
        }
        for item in &empty.stale_followups {
            assert_eq!(item.usage, zero);
        }
        assert_eq!(reads_marker(&zero), "");
        assert_eq!(reads_marker(&usage::PageUsage::default()), "");
        let usage = BTreeMap::from([(
            "d".into(),
            usage::PageUsage {
                reads: 5,
                read_sessions: 2,
                last_read: Some(123),
                injected_unread: 1,
                importance: 1.5,
            },
        )]);
        let mut ranked = original;
        prioritize(&mut ranked, &usage);
        assert_eq!(
            ranked
                .orphans
                .iter()
                .map(|i| i.path.as_str())
                .collect::<Vec<_>>(),
            ["d", "a", "b", "e"]
        );
        assert_eq!(
            ranked
                .duplicates
                .iter()
                .map(|i| i.a.as_str())
                .collect::<Vec<_>>(),
            ["c", "e", "a"]
        );
        let expected = &usage["d"];
        assert_eq!(&ranked.duplicates[0].usage_b, expected);
        assert_eq!(&ranked.orphans[0].usage, expected);
        let unread = ranked.unread.as_ref().unwrap();
        assert_eq!(unread[0].path, "a");
        assert_eq!(&unread[1].usage, expected);
        assert_eq!(ranked.stale_followups[0].path, "a");
        assert_eq!(&ranked.stale_followups[1].usage, expected);
        assert_eq!(reads_marker(expected), " · 5 reads");
    }

    #[test]
    fn report_json_preserves_key_order_and_optional_unread() -> Result<()> {
        let report = Report {
            broken_links: vec![Finding {
                detail: "missing".into(),
                path: "a".into(),
                usage: usage::PageUsage {
                    importance: 1.5,
                    injected_unread: 1,
                    last_read: Some(123),
                    read_sessions: 2,
                    reads: 5,
                },
            }],
            duplicates: vec![Duplicate {
                a: "a".into(),
                b: "b".into(),
                score: 0.99,
                ..Duplicate::default()
            }],
            singleton_tags: vec![SingletonTag {
                path: "b".into(),
                tag: "tag".into(),
                ..SingletonTag::default()
            }],
            stale_followups: vec![StaleFollowup {
                age_days: 30,
                mtime: 123,
                path: "c".into(),
                ..StaleFollowup::default()
            }],
            ..Report::default()
        };
        assert_eq!(
            serde_json::to_string(&report)?,
            concat!(
                r#"{"broken-links":[{"detail":"missing","path":"a","usage":{"importance":1.5,"injected_unread":1,"last_read":123,"read_sessions":2,"reads":5}}],"#,
                r#""duplicates":[{"a":"a","b":"b","score":0.99,"usage_a":{"importance":0.0,"injected_unread":0,"last_read":null,"read_sessions":0,"reads":0},"usage_b":{"importance":0.0,"injected_unread":0,"last_read":null,"read_sessions":0,"reads":0}}],"#,
                r#""frontmatter":[],"log-growth":[],"missing-queries":[],"orphans":[],"#,
                r#""singleton-tags":[{"path":"b","tag":"tag","usage":{"importance":0.0,"injected_unread":0,"last_read":null,"read_sessions":0,"reads":0}}],"snapshots":[],"#,
                r#""stale-followups":[{"age_days":30,"mtime":123,"path":"c","usage":{"importance":0.0,"injected_unread":0,"last_read":null,"read_sessions":0,"reads":0}}]}"#,
            )
        );
        assert_eq!(
            serde_json::to_string(&Report {
                unread: Some(vec![]),
                ..Report::default()
            })?,
            r#"{"broken-links":[],"duplicates":[],"frontmatter":[],"log-growth":[],"missing-queries":[],"orphans":[],"singleton-tags":[],"snapshots":[],"stale-followups":[],"unread":[]}"#
        );
        Ok(())
    }
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
        assert!(initial.unread.is_none());
        for (category, count) in [
            ("snapshots", initial.snapshots.len()),
            ("log-growth", initial.log_growth.len()),
            ("stale-followups", initial.stale_followups.len()),
            ("orphans", initial.orphans.len()),
            ("frontmatter", initial.frontmatter.len()),
            ("broken-links", initial.broken_links.len()),
            ("missing-queries", initial.missing_queries.len()),
            ("singleton-tags", initial.singleton_tags.len()),
        ] {
            assert_eq!(count, 1, "{category}: {initial:?}");
        }
        assert_eq!(initial, worklist(&wiki.root, &wiki.index, now)?);
        let usage = usage::open(&wiki.root)?;
        usage.execute_batch("insert into state values ('scanned', 1), ('scan_format', 2); insert into tools values ('r','s',1); insert into accesses values ('r','pages/knowledge/linked.md','read');")?;
        let after = worklist(&wiki.root, &wiki.index, now)?;
        assert_eq!(after.unread.as_ref().unwrap().len(), 3);
        assert_eq!(after.stale_followups[0].age_days, 20);
        Ok(())
    }
}

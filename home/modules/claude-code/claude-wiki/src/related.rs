use crate::{
    context,
    index::{Row, COLUMNS, JOIN},
    pages, semantic, usage,
};
use anyhow::Result;
use clap::Args as ClapArgs;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use std::{
    collections::HashSet,
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(ClapArgs)]
pub struct Args {
    #[arg(long)]
    pub json: bool,
    #[arg(short, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=10))]
    pub n: u8,
    #[arg(long, default_value = ".")]
    pub cwd: PathBuf,
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long, value_parser = usage::source)]
    pub source: Option<String>,
    #[arg(long, default_value_t = 1500)]
    pub timeout_ms: u64,
}

#[derive(Debug, Serialize)]
struct Page {
    path: PathBuf,
    rel: String,
    title: String,
    #[serde(rename = "type")]
    kind: String,
    section: Option<semantic::Section>,
}

#[derive(Debug, Serialize)]
struct Report {
    pages: Vec<Page>,
    overview: Option<PathBuf>,
}

fn select(root: &Path, db: &Connection, hits: Vec<semantic::Hit>, n: u8) -> Result<Vec<Page>> {
    let mut selected = vec![];
    let mut seen = HashSet::new();
    for hit in hits {
        let Some(rel) = pages::link_path(&hit.path) else {
            continue;
        };
        if !seen.insert(rel.clone()) {
            continue;
        }
        let row = db
            .query_row(
                &format!("select {COLUMNS} {JOIN} where files.path=? and fts.type!='history' and files.blocked=''"),
                [&rel],
                Row::read,
            )
            .optional()?;
        if let Some(row) = row {
            selected.push(Page {
                path: root.join(&rel),
                rel,
                title: row.title,
                kind: row.kind,
                section: hit.section.map(Into::into),
            });
            if selected.len() == usize::from(n) {
                break;
            }
        }
    }
    Ok(selected)
}

fn report(root: &Path, db: &Connection, args: &Args, hits: Vec<semantic::Hit>) -> Result<Report> {
    let root = root.canonicalize()?;
    let pages = select(&root, db, hits, args.n)?;
    let overview = if let Some(project) = context::current_project(&args.cwd) {
        let rel = format!("pages/projects/{project}.md");
        let exists: bool = db.query_row(
            "select exists(select 1 from files where path=? and blocked='')",
            [&rel],
            |r| r.get(0),
        )?;
        let path = root.join(rel);
        (exists && path.is_file()).then_some(path)
    } else {
        None
    };
    if let (Some(session), Some(source)) = (&args.session, &args.source) {
        if !pages.is_empty() {
            usage::log(
                &root,
                session,
                source,
                &pages
                    .iter()
                    .map(|page| page.rel.clone())
                    .collect::<Vec<_>>(),
            );
        }
    }
    Ok(Report { pages, overview })
}

fn read_prompt(input: impl Read) -> io::Result<String> {
    let mut bytes = vec![];
    input.take(16 * 1024).read_to_end(&mut bytes)?;
    // A byte cap may split a UTF-8 character; keep only the valid prefix in that case.
    let end = std::str::from_utf8(&bytes)
        .err()
        .map_or(bytes.len(), |e| e.valid_up_to());
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

pub fn run(root: &Path, db: &Connection, args: &Args) -> Result<()> {
    let prompt = read_prompt(io::stdin().lock())?;
    let hits = semantic::related_with_timeout(&prompt, Duration::from_millis(args.timeout_ms));
    let report = report(root, db, args, hits)?;
    if args.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for page in report.pages {
            println!("{} ({})", page.path.display(), page.title);
        }
        if let Some(path) = report.overview {
            println!("overview: {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;

    fn args(cwd: &Path) -> Args {
        Args {
            json: true,
            n: 3,
            cwd: cwd.into(),
            session: None,
            source: None,
            timeout_ms: 1500,
        }
    }

    fn hits() -> Vec<semantic::Hit> {
        [
            "history/past",
            "knowledge/blocked",
            "knowledge/missing",
            "../outside",
            "knowledge/a",
            "knowledge/a",
            "knowledge/b",
            "knowledge/c",
            "knowledge/d",
        ]
        .into_iter()
        .map(|path| semantic::Hit {
            path: path.into(),
            section: Some(("Details".into(), 1, 9)),
        })
        .collect()
    }

    #[test]
    fn filtering_limit_and_json_shape() -> Result<()> {
        let mut wiki = usage::tests::Wiki::new()?;
        for name in [
            "history/past",
            "knowledge/blocked",
            "knowledge/a",
            "knowledge/b",
            "knowledge/c",
            "knowledge/d",
        ] {
            wiki.page(
                &format!("pages/{name}.md"),
                "---\ntitle: Test\ncreated: 2020-01-01\n---\nbody",
            )?;
        }
        wiki.index.execute(
            "update files set blocked='secret' where path='pages/knowledge/blocked.md'",
            [],
        )?;
        for (n, count) in [(1, 1), (3, 3), (10, 4)] {
            let pages = select(&wiki.root, &wiki.index, hits(), n)?;
            assert_eq!(pages.len(), count);
            assert_eq!(
                serde_json::to_value(&pages[0])?,
                json!({
                    "path": wiki.root.join("pages/knowledge/a.md"), "rel": "pages/knowledge/a.md",
                    "title": "Test", "type": "knowledge", "section": {"heading": "Details", "start": 1, "end": 9}
                })
            );
        }
        let pages = select(
            &wiki.root,
            &wiki.index,
            vec![semantic::Hit {
                path: "knowledge/a".into(),
                section: None,
            }],
            1,
        )?;
        assert!(serde_json::to_value(&pages[0])?["section"].is_null());
        assert_eq!(
            serde_json::to_value(report(&wiki.root, &wiki.index, &args(&wiki.root), vec![])?)?,
            json!({"pages": [], "overview": null})
        );
        Ok(())
    }

    #[test]
    fn overview_and_logging_only_selected_pages_with_both_options() -> Result<()> {
        let mut wiki = usage::tests::Wiki::new()?;
        let project = context::current_project(&wiki.root).unwrap();
        let overview = format!("pages/projects/{project}.md");
        wiki.page(
            &overview,
            "---\ntitle: Overview\ncreated: 2020-01-01\n---\nbody",
        )?;
        wiki.page(
            "pages/knowledge/a.md",
            "---\ntitle: Test\ncreated: 2020-01-01\n---\nbody",
        )?;
        let mut args = args(&wiki.root);
        for (session, source) in [
            (None, None),
            (Some("session"), None),
            (None, Some("mod-agent")),
        ] {
            args.session = session.map(str::to_owned);
            args.source = source.map(str::to_owned);
            let report = report(&wiki.root, &wiki.index, &args, hits())?;
            assert_eq!(report.overview, Some(wiki.root.join(&overview)));
            assert!(!wiki.root.join("usage.sqlite").exists());
        }
        args.session = Some("session".into());
        args.source = Some("mod-agent".into());
        report(&wiki.root, &wiki.index, &args, vec![])?;
        assert!(!wiki.root.join("usage.sqlite").exists());
        report(&wiki.root, &wiki.index, &args, hits())?;
        let usage = usage::open(&wiki.root)?;
        let logged: Vec<(String, String, String)> = usage
            .prepare("select session_id, source, path from injections")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        assert_eq!(
            logged,
            [(
                "session".into(),
                "mod-agent".into(),
                "pages/knowledge/a.md".into()
            )]
        );
        wiki.index.execute(
            "update files set blocked='secret' where path=?",
            [&overview],
        )?;
        assert!(report(&wiki.root, &wiki.index, &args, vec![])?
            .overview
            .is_none());
        std::fs::remove_file(wiki.root.join(&overview))?;
        crate::index::refresh(&wiki.root, &mut wiki.index)?;
        assert!(report(&wiki.root, &wiki.index, &args, vec![])?
            .overview
            .is_none());
        args.cwd = wiki.root.join("pages"); // Inside the same repository.
        assert_eq!(
            context::current_project(&args.cwd).as_deref(),
            Some(project.as_str())
        );
        args.cwd = wiki.root.join("absent");
        assert!(report(&wiki.root, &wiki.index, &args, vec![])?
            .overview
            .is_none());
        Ok(())
    }

    #[test]
    fn source_validation_and_cli_defaults() {
        for value in ["a", "mod-agent", &"a".repeat(31)] {
            assert!(usage::source(value).is_ok());
        }
        for value in [
            "",
            "A",
            "-a",
            "agent_foo",
            "a1",
            "a b",
            "a\n",
            "é",
            &"a".repeat(32),
        ] {
            assert!(usage::source(value).is_err());
            let err = crate::Cli::try_parse_from(["claude-wiki", "related", "--source", value])
                .err()
                .unwrap();
            assert_eq!(err.exit_code(), 2);
        }
        for value in ["0", "11", "-1"] {
            assert_eq!(
                crate::Cli::try_parse_from(["claude-wiki", "related", "-n", value])
                    .err()
                    .unwrap()
                    .exit_code(),
                2
            );
        }
        let cli = crate::Cli::try_parse_from(["claude-wiki", "related"]).unwrap();
        let crate::Commands::Related(args) = cli.command else {
            panic!("wrong command")
        };
        assert_eq!(args.n, 3);
        assert_eq!(args.timeout_ms, 1500);
        assert_eq!(args.cwd, Path::new("."));
    }

    #[test]
    fn input_is_capped_without_splitting_utf8() -> Result<()> {
        let text = format!("{}한more", "a".repeat(16 * 1024 - 1));
        let mut input = text.as_bytes();
        assert_eq!(read_prompt(&mut input)?, "a".repeat(16 * 1024 - 1));
        assert_eq!(input.len(), text.len() - 16 * 1024);
        assert_eq!(
            read_prompt("free text\n한글".as_bytes())?,
            "free text\n한글"
        );
        Ok(())
    }
}

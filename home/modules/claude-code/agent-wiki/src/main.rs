//! Shared markdown wiki, indexed incrementally and serialized across sessions.
mod antigravity;
mod codex_recall;
mod compat;
mod context;
mod hook_payload;
mod index;
mod injections;
mod links;
mod lint;
mod pages;
mod recall;
mod records;
mod related;
mod remind;
mod remind_event;
mod scan;
mod search;
mod semantic;
mod sessions;
mod sync;
mod transcript;
mod usage;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(
    name = "agent-wiki",
    bin_name = "agent-wiki",
    about = "Shared markdown wiki for Claude Code and Codex sessions, searched with SQLite FTS5 (BM25)."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fused BM25 and dense search; words are stemmed, prefix-matched and OR-ed
    Search {
        #[arg(required = true)]
        terms: Vec<String>,
        #[arg(short, default_value_t = 10)]
        n: i64,
        #[command(flatten)]
        filters: search::Filters,
        /// Use only BM25 ranking, with history pages down-weighted
        #[arg(long)]
        no_semantic: bool,
        #[arg(long)]
        json: bool,
    },
    /// Semantic suggestions from stdin, plus the current project's overview
    Related(related::Args),
    /// Pages injected into a session, newest first
    Injections(injections::Args),
    /// Wiki pages matching a skill, optionally deduplicated within a session
    SkillPages(recall::SkillPagesArgs),
    /// Most recently updated pages
    List {
        #[arg(short, default_value_t = 20)]
        n: i64,
        #[command(flatten)]
        filters: search::Filters,
    },
    /// Tags and projects in use, with page counts
    Tags,
    /// Wiki injection and transcript access counters
    Stats(usage::Args),
    /// Deterministic wiki maintenance worklist
    Lint(lint::Args),
    /// Outgoing links and backlinks for a page
    Links { page: String },
    /// Index pages and commit changes to the wiki's git repo
    Sync {
        paths: Vec<String>,
        #[arg(short, long)]
        message: Option<String>,
        #[arg(long)]
        rebuild: bool,
    },
    /// Validate pages without committing
    Check {
        paths: Vec<String>,
        /// Test each page's queries against BM25 and semantic search
        #[arg(long)]
        probe: bool,
        /// A query must find its page within this many results by either method
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(i64).range(1..), requires = "probe")]
        probe_top: i64,
    },
    /// Session start summary
    Context(context::Args),
    /// BM25 search over past session transcripts
    Sessions(sessions::Args),
    /// Save a deterministic local session record (hook stdin or batch backfill)
    Record(records::Args),
    /// Periodic UserPromptSubmit reminder, or a reminder after a meaningful event
    Remind(remind::Args),
    /// Suggest wiki pages for skill calls or exact error text on failures
    Recall(recall::Args),
}

fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn home() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| PathBuf::from("."))
}

fn expand_home(path: &Path) -> PathBuf {
    if path == Path::new("~") {
        home()
    } else if let Ok(tail) = path.strip_prefix("~/") {
        home().join(tail)
    } else {
        path.to_path_buf()
    }
}

fn wiki_root() -> PathBuf {
    crate::compat::var_os("AGENT_WIKI_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .map(|p| expand_home(&p))
        .unwrap_or_else(|| {
            crate::compat::directory(
                &env_path("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local/share")),
            )
        })
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git -C {} {} failed:\n{}",
            root.display(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn ensure_layout(root: &Path) -> Result<()> {
    for kind in pages::TYPES {
        fs::create_dir_all(root.join("pages").join(kind))?;
    }
    let ignore = root.join(".gitignore");
    let mut text = match fs::read_to_string(&ignore) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "index.sqlite*\n.lock\n".into(),
        Err(e) => return Err(e.into()),
    };
    // wiki-embed writes vectors-<model>.npz here; like the indexes it is derived from
    // the pages and rebuilt in seconds, so it never belongs in history.
    for derived in ["sessions.sqlite*", "usage.sqlite*", "vectors-*.npz"] {
        if !text.lines().any(|s| s == derived) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(derived);
            text.push('\n');
        }
    }
    if fs::read_to_string(&ignore).ok().as_ref() != Some(&text) {
        fs::write(ignore, text)?;
    }
    if !root.join(".git").exists() {
        git(root, &["init", "-q"])?;
        // Hooks commit unattended and must not depend on a signing key.
        git(root, &["config", "commit.gpgsign", "false"])?;
    }
    Ok(())
}

fn run(command: &Commands) -> Result<i32> {
    if let Commands::Record(args) = command {
        records::run(args)?;
        return Ok(0);
    }
    if let Commands::Sessions(args) = command
        && args.records
    {
        records::search(args)?;
        return Ok(0);
    }
    let root = wiki_root();
    if let Commands::Remind(args) = command
        && args.event
    {
        // Event hooks only need reminder state: do not refresh the wiki or query embeddings.
        remind_event::run(&root, args)?;
        return Ok(0);
    }
    if matches!(
        command,
        Commands::Context(_) | Commands::Remind(_) | Commands::Recall(_)
    ) {
        return Ok(
            with_hook_lock(&root, Duration::MAX, || run_locked(&root, command))?.unwrap_or(0),
        );
    }
    let mut lock = open_lock(&root)?;
    let _guard = lock.write()?;
    run_locked(&root, command)
}

fn open_lock(root: &Path) -> Result<fd_lock::RwLock<fs::File>> {
    fs::create_dir_all(root)?;
    Ok(fd_lock::RwLock::new(
        fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(".lock"))?,
    ))
}

/// Hooks must not stall a session behind another wiki process: wait for the wiki
/// lock at most `AGENT_WIKI_HOOK_LOCK_MS` (default 2000, capped by `max`), then
/// give up with `None` and let the hook produce nothing.
fn with_hook_lock<T>(
    root: &Path,
    max: Duration,
    f: impl FnOnce() -> Result<T>,
) -> Result<Option<T>> {
    let mut lock = open_lock(root)?;
    let timeout = Duration::from_millis(
        crate::compat::var("AGENT_WIKI_HOOK_LOCK_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2000),
    )
    .min(max);
    let started = Instant::now();
    loop {
        match lock.try_write() {
            Ok(_guard) => return f().map(Some),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Ok(None);
                }
                thread::sleep(remaining.min(Duration::from_millis(25)));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn run_locked(root: &Path, command: &Commands) -> Result<i32> {
    ensure_layout(root)?;
    if matches!(command, Commands::Sync { rebuild: true, .. }) {
        index::remove_index(root, "index.sqlite")?;
    }
    let mut db = index::open(root)?;
    index::refresh(root, &mut db)?;
    match command {
        Commands::Search {
            terms,
            n,
            filters,
            no_semantic,
            json,
        } => search::search(root, &db, terms, *n, filters, *no_semantic, *json)?,
        Commands::Related(args) => related::run(root, &db, args)?,
        Commands::Injections(args) => injections::run(root, &db, args)?,
        Commands::SkillPages(args) => recall::run_skill_pages(root, &db, args)?,
        Commands::List { n, filters } => search::list(root, &db, *n, filters)?,
        Commands::Stats(args) => usage::run(root, &db, args)?,
        Commands::Lint(args) => lint::run(root, &db, args)?,
        Commands::Tags => search::tags(&db)?,
        Commands::Links { page } => links::show(root, &db, page)?,
        Commands::Sync { paths, message, .. } => {
            return sync::sync(root, &db, paths, message.as_deref());
        }
        Commands::Check {
            paths,
            probe,
            probe_top,
        } => return sync::check(root, &db, paths, *probe, *probe_top),
        Commands::Context(args) => context::run(root, &db, args)?,
        Commands::Sessions(args) => sessions::run(root, args)?,
        Commands::Record(_) => unreachable!(),
        Commands::Remind(args) => remind::run(root, &db, args)?,
        Commands::Recall(args) => recall::run(root, &db, args)?,
    }
    Ok(0)
}

fn main() {
    let argv: Vec<_> = env::args().collect();
    let antigravity = argv
        .get(1)
        .is_some_and(|s| matches!(s.as_str(), "context" | "recall" | "remind" | "record"))
        && (argv.windows(2).any(|s| s == ["--harness", "antigravity"])
            || argv.iter().any(|s| s == "--harness=antigravity"));
    // Even malformed hook arguments must not interrupt a session.
    let is_hook = env::args_os().nth(1).is_some_and(|s| {
        s == "remind"
            || s == "recall"
            || s == "context"
            || (s == "record" && !env::args_os().any(|s| s == "--backfill" || s == "--wait"))
    });
    let cli = Cli::try_parse().unwrap_or_else(|e| {
        if antigravity {
            // Still answer a PreToolUse payload with its required decision.
            let input = antigravity::read_input().unwrap_or_default();
            println!(
                "{}",
                antigravity::output(&antigravity::event_name(&input), "")
            );
        }
        if is_hook {
            eprintln!("{e}");
        } else {
            let _ = e.print();
        }
        std::process::exit(if is_hook { 0 } else { e.exit_code() });
    });
    if antigravity {
        // One JSON reply on every path, including malformed payloads, busy locks
        // and panics. Stop acknowledges before transcript IO or detached work.
        let record = matches!(cli.command, Commands::Record(_));
        if record {
            use std::io::Write;
            println!("{{}}");
            let _ = io::stdout().flush();
        }
        let mut event = String::new();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &cli.command {
                Commands::Record(args) => records::run(args).map(|_| serde_json::json!({})),
                command => {
                    let input = antigravity::read_input()?;
                    event = antigravity::event_name(&input);
                    antigravity::run(command, &input)
                }
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("Antigravity hook failed")));
        let failed = result.is_err();
        let output = result.unwrap_or_else(|e| {
            eprintln!("agent-wiki: {e:#}");
            // Error fallback still carries PreToolUse's required decision.
            antigravity::output(&event, "")
        });
        if !record {
            println!("{output}");
        }
        std::process::exit(if is_hook || !failed { 0 } else { 1 });
    }
    let result = if is_hook {
        std::panic::catch_unwind(|| run(&cli.command))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("hook failed")))
    } else {
        run(&cli.command)
    };
    let code = match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("agent-wiki: {e:#}");
            1
        }
    };
    std::process::exit(if is_hook { 0 } else { code });
}

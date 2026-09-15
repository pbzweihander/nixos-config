//! Shared markdown wiki, indexed incrementally and serialized across sessions.
mod context;
mod index;
mod pages;
mod remind;
mod scan;
mod search;
mod sessions;
mod sync;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
#[command(
    about = "Shared markdown wiki for Claude Code sessions, searched with SQLite FTS5 (BM25)."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// BM25 search; words are stemmed, prefix-matched and OR-ed
    Search {
        #[arg(required = true)]
        terms: Vec<String>,
        #[arg(short, default_value_t = 10)]
        n: i64,
        #[command(flatten)]
        filters: search::Filters,
    },
    /// Most recently updated pages
    List {
        #[arg(short, default_value_t = 20)]
        n: i64,
        #[command(flatten)]
        filters: search::Filters,
    },
    /// Tags and projects in use, with page counts
    Tags,
    /// Index pages and commit changes to the wiki's git repo
    Sync {
        paths: Vec<String>,
        #[arg(short, long)]
        message: Option<String>,
        #[arg(long)]
        rebuild: bool,
    },
    /// Validate pages without committing
    Check { paths: Vec<String> },
    /// Session start summary
    Context(context::Args),
    /// BM25 search over past session transcripts
    Sessions(sessions::Args),
    /// Periodic UserPromptSubmit reminder
    Remind(remind::Args),
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
    env_path("CLAUDE_WIKI_DIR")
        .map(|p| expand_home(&p))
        .unwrap_or_else(|| {
            env_path("XDG_DATA_HOME")
                .unwrap_or_else(|| home().join(".local/share"))
                .join("claude-wiki")
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
    if !text.lines().any(|s| s == "sessions.sqlite*") {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("sessions.sqlite*\n");
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
    let root = wiki_root();
    fs::create_dir_all(&root)?;
    let mut lock = fd_lock::RwLock::new(
        fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(".lock"))?,
    );
    let _guard = lock.write()?;
    ensure_layout(&root)?;
    if matches!(command, Commands::Sync { rebuild: true, .. }) {
        index::remove_index(&root, "index.sqlite")?;
    }
    let mut db = index::open(&root)?;
    index::refresh(&root, &mut db)?;
    match command {
        Commands::Search { terms, n, filters } => search::search(&root, &db, terms, *n, filters)?,
        Commands::List { n, filters } => search::list(&root, &db, *n, filters)?,
        Commands::Tags => search::tags(&db)?,
        Commands::Sync { paths, message, .. } => {
            return sync::sync(&root, paths, message.as_deref())
        }
        Commands::Check { paths } => return sync::check(&root, paths),
        Commands::Context(args) => context::run(&root, &db, args)?,
        Commands::Sessions(args) => sessions::run(&root, args)?,
        Commands::Remind(args) => remind::run(&db, args)?,
    }
    Ok(0)
}

fn main() {
    // Even malformed hook arguments must not prevent a prompt from being submitted.
    let is_remind = env::args_os().nth(1).is_some_and(|s| s == "remind");
    let cli = Cli::try_parse().unwrap_or_else(|e| {
        let _ = e.print();
        std::process::exit(if is_remind { 0 } else { e.exit_code() });
    });
    let result = if is_remind {
        std::panic::catch_unwind(|| run(&cli.command))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("reminder failed")))
    } else {
        run(&cli.command)
    };
    let code = match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("claude-wiki: {e:#}");
            1
        }
    };
    std::process::exit(if is_remind { 0 } else { code });
}

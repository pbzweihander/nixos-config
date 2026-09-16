use crate::{context, expand_home, git, index, pages, scan};
use anyhow::{bail, Result};
use rusqlite::Connection;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

fn resolve(path: &Path) -> Result<PathBuf> {
    // Resolve existing prefixes too: deleted files still need path-scoped commits,
    // while a symlink must not turn an apparently local path into an outside one.
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => {
                out.push(other.as_os_str());
                match fs::symlink_metadata(&out) {
                    Ok(_) => out = fs::canonicalize(&out)?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    Ok(out)
}

pub fn wiki_paths(root: &Path, paths: &[String]) -> Result<Vec<String>> {
    let root_abs = fs::canonicalize(root)?;
    paths
        .iter()
        .map(|p| {
            // Accept the `type/slug` form that links and `links` output use, so a
            // path copied from there works for sync and check as well.
            let p = pages::link_path(p).unwrap_or_else(|| p.to_owned());
            let path = expand_home(Path::new(&p));
            let path = resolve(&if path.is_absolute() {
                path
            } else {
                root_abs.join(path)
            })?;
            let Ok(rel) = path.strip_prefix(&root_abs) else {
                bail!("{p} is not inside the wiki ({})", root.display());
            };
            Ok(if rel.as_os_str().is_empty() {
                ".".into()
            } else {
                rel.to_string_lossy().into_owned()
            })
        })
        .collect()
}

fn validate(root: &Path, rel: &str, text: &str) -> i32 {
    let page = pages::parse(text, rel);
    let mut code = 0;
    if pages::page_type(rel).is_empty() {
        eprintln!(
            "warning: {rel} is not directly under pages/<{}>/",
            pages::TYPES.join("|")
        );
        code = 1;
    }
    for problem in page.problems {
        eprintln!("warning: {rel} {}", scan::redact(&problem));
        code = 1;
    }
    for target in page.links {
        if !root.join(&target).is_file() {
            eprintln!("warning: {rel}: link to {target}, which does not exist");
            code = 1;
        }
    }
    let budget = context::overview_budget();
    let length = page.body.chars().count();
    if page.kind == "projects" && length > budget {
        eprintln!("warning: {rel}: body is {length} chars; only the first {budget} are shown at session start");
        code = 1;
    }
    for finding in scan::scan(text) {
        eprintln!("blocked: {rel}: {}: {}", finding.category, finding.excerpt);
        code = 2;
    }
    code
}

pub fn check(root: &Path, paths: &[String]) -> Result<i32> {
    let paths = if paths.is_empty() {
        vec!["pages".into()]
    } else {
        wiki_paths(root, paths)?
    };
    let mut selected = vec![];
    for rel in paths {
        let path = root.join(rel);
        if path.is_dir() {
            selected.extend(pages::markdown_files(&path)?);
        } else if path.exists() {
            selected.push(path);
        }
    }
    selected.sort();
    selected.dedup();
    let mut code = 0;
    for path in selected {
        code = code.max(validate(
            root,
            &path.strip_prefix(root)?.to_string_lossy(),
            &pages::read(&path)?,
        ));
    }
    Ok(code)
}

pub fn sync(root: &Path, db: &Connection, paths: &[String], message: Option<&str>) -> Result<i32> {
    let rels = wiki_paths(root, paths)?;
    let pathspec: Vec<&str> = if paths.is_empty() {
        vec![]
    } else {
        std::iter::once("--")
            .chain(rels.iter().map(String::as_str))
            .collect()
    };
    let run = |args: &[&str]| {
        let mut args = args.to_vec();
        args.extend(&pathspec);
        git(root, &args)
    };
    run(&["add", "-A"])?;
    let changes = run(&["diff", "--cached", "--name-status", "-z"])?;
    let fields: Vec<_> = changes.split('\0').filter(|s| !s.is_empty()).collect();
    let mut changes = vec![];
    let mut removed = vec![];
    let mut i = 0;
    while i < fields.len() {
        let status = fields[i];
        let count = if status.starts_with(['R', 'C']) { 2 } else { 1 };
        if status.starts_with(['D', 'R']) {
            removed.push(fields[i + 1]);
        }
        changes.push((status, fields[i + count]));
        i += count + 1;
    }
    if changes.is_empty() {
        println!("nothing to commit");
        return Ok(0);
    }
    let mut code = 0;
    for (status, rel) in &changes {
        if status.starts_with('D') || !rel.ends_with(".md") {
            continue;
        }
        // Validate exactly the staged content that a commit without paths uses.
        let text = git(root, &["show", &format!(":{rel}")])?;
        code = code.max(validate(root, rel, &text));
    }
    if code == 2 {
        return Ok(2);
    }
    for rel in removed {
        let sources = index::linked_from(db, rel)?;
        if !sources.is_empty() {
            eprintln!(
                "warning: {rel} is linked from {} pages: {}",
                sources.len(),
                sources.join(", ")
            );
        }
    }
    let summary: Vec<_> = changes
        .iter()
        .map(|(status, rel)| {
            let verb = match status.chars().next().unwrap_or(' ') {
                'A' => "add",
                'M' => "update",
                'D' => "delete",
                'R' => "rename",
                'C' => "copy",
                _ => status,
            };
            let rel = rel.strip_prefix("pages/").unwrap_or(rel);
            format!("{verb} {}", rel.strip_suffix(".md").unwrap_or(rel))
        })
        .collect();
    let default = format!(
        "wiki: {}{}",
        summary
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        if summary.len() > 5 {
            format!(" (+{} more)", summary.len() - 5)
        } else {
            String::new()
        }
    );
    let message = message.filter(|s| !s.is_empty()).unwrap_or(&default);
    run(&["commit", "-q", "-m", message, "-m", &summary.join("\n")])?;
    println!("committed: {message}");
    if !pathspec.is_empty() {
        let others = git(root, &["status", "--porcelain"])?;
        let others: Vec<_> = others.lines().filter_map(|line| line.get(3..)).collect();
        if !others.is_empty() {
            println!(
                "left uncommitted (other sessions' changes): {}",
                others.join(", ")
            );
        }
    }
    Ok(code)
}

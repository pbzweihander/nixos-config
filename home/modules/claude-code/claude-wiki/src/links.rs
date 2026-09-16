use crate::{index, sync};
use anyhow::{bail, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

fn title(db: &Connection, path: &str) -> Result<String> {
    let row = db
        .query_row(
            &format!(
                "select {} {} where files.path = ?",
                index::COLUMNS,
                index::JOIN
            ),
            [path],
            index::Row::read,
        )
        .optional()?;
    Ok(match row {
        None => "[missing]".into(),
        Some(row) if !row.blocked.is_empty() => format!("[BLOCKED: {}]", row.blocked),
        Some(row) => row.title,
    })
}

pub fn show(root: &Path, db: &Connection, page: &str) -> Result<()> {
    let target = page.strip_prefix("[[").and_then(|p| p.strip_suffix("]]"));
    let rel = sync::wiki_paths(root, &[target.unwrap_or(page).to_owned()])?.remove(0);
    if !db.query_row(
        "select exists(select 1 from files where path = ?)",
        [&rel],
        |r| r.get::<_, bool>(0),
    )? {
        bail!("no such page");
    }
    let outgoing = db
        .prepare("select to_path from links join files on files.id = links.from_id where files.path = ? order by to_path")?
        .query_map([&rel], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    println!("{rel}");
    for (heading, paths) in [
        ("links to:", outgoing),
        ("linked from:", index::linked_from(db, &rel)?),
    ] {
        println!("{heading}");
        if paths.is_empty() {
            println!("(none)");
        }
        for path in paths {
            println!("- {path}: {}", title(db, &path)?);
        }
    }
    Ok(())
}

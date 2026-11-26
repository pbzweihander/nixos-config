use crate::{index, sync};
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

/// What to show for a linked page: its title, or why there is none to show.
fn title(db: &Connection, path: &str) -> Result<String> {
    // Only the two columns this needs: `index::COLUMNS` would count backlinks for
    // every neighbour just to drop the number.
    let row: Option<(String, String)> = db
        .prepare_cached(
            "select fts.title, files.blocked from fts join files on files.id = fts.rowid
             where files.path = ?",
        )?
        .query_row([path], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(match row {
        None => "[missing]".into(),
        Some((_, blocked)) if !blocked.is_empty() => format!("[BLOCKED: {blocked}]"),
        Some((title, _)) => title,
    })
}

/// The page a `links` argument names: a path, `type/slug`, or a wikilink as it is
/// written in a page, `[[type/slug]]` or `[[type/slug|label]]`.
fn target(page: &str) -> &str {
    match page.strip_prefix("[[").and_then(|p| p.strip_suffix("]]")) {
        Some(link) => link.split('|').next().unwrap_or(link),
        None => page,
    }
}

pub fn show(root: &Path, db: &Connection, page: &str) -> Result<()> {
    let rel = sync::wiki_paths(root, &[target(page).to_owned()])?.remove(0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wikilink_argument_loses_its_brackets_and_its_label() {
        for (page, expected) in [
            ("knowledge/foo", "knowledge/foo"),
            ("[[knowledge/foo]]", "knowledge/foo"),
            ("[[knowledge/foo|the foo page]]", "knowledge/foo"),
            ("[[knowledge/foo|a|b]]", "knowledge/foo"),
            ("pages/knowledge/foo.md", "pages/knowledge/foo.md"),
            // Only a wikilink has a label: a bare path keeps its bar.
            ("knowledge/foo|label", "knowledge/foo|label"),
            ("[[knowledge/foo", "[[knowledge/foo"),
        ] {
            assert_eq!(target(page), expected, "{page}");
        }
    }

    #[test]
    fn links_of_an_aliased_wikilink_and_titles_of_neighbours() -> Result<()> {
        let mut wiki = crate::usage::tests::Wiki::new()?;
        wiki.page(
            "pages/knowledge/target.md",
            "---\ntitle: Target page\ncreated: 2026-11-27\n---\n본문\n",
        )?;
        wiki.page(
            "pages/knowledge/hidden.md",
            &format!(
                "---\ntitle: Hidden title\ncreated: 2026-11-27\n---\nghp_{}\n",
                "A".repeat(36)
            ),
        )?;
        wiki.page(
            "pages/knowledge/source.md",
            "---\ntitle: Source\ncreated: 2026-11-27\n---\n\
             [[knowledge/target|대상]] [[knowledge/hidden]] [[knowledge/absent]]\n",
        )?;
        for page in [
            "[[knowledge/source|label]]",
            "[[knowledge/source]]",
            "knowledge/source",
        ] {
            show(&wiki.root, &wiki.index, page)?;
        }
        assert!(show(&wiki.root, &wiki.index, "[[knowledge/absent|label]]").is_err());
        assert_eq!(
            title(&wiki.index, "pages/knowledge/target.md")?,
            "Target page"
        );
        assert_eq!(
            title(&wiki.index, "pages/knowledge/hidden.md")?,
            "[BLOCKED: secret]"
        );
        assert_eq!(
            title(&wiki.index, "pages/knowledge/absent.md")?,
            "[missing]"
        );
        Ok(())
    }
}

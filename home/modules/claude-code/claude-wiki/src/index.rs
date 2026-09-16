use crate::{pages, scan};
use anyhow::Result;
use rusqlite::{params, Connection};
use std::{collections::HashMap, fs, os::unix::fs::MetadataExt, path::Path, time::Duration};

const SCHEMA_VERSION: i64 = 4;
pub const COLUMNS: &str = "fts.path, fts.type, fts.status, fts.title, fts.project, fts.tags, files.mtime_ns, files.blocked, (select count(*) from links where links.to_path = files.path) as backlinks";
pub const JOIN: &str = "from fts join files on files.id = fts.rowid";

pub fn remove_index(root: &Path, name: &str) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(name) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

pub fn open(root: &Path) -> Result<Connection> {
    let db = Connection::open(root.join("index.sqlite"))?;
    db.busy_timeout(Duration::from_secs(30))?;
    initialize(&db)?;
    Ok(db)
}

fn initialize(db: &Connection) -> Result<()> {
    if db.query_row("pragma user_version", [], |r| r.get::<_, i64>(0))? != SCHEMA_VERSION {
        db.execute_batch(&format!("begin;
            drop table if exists files; drop table if exists fts; drop table if exists links;
            create table files(id integer primary key, path text unique, mtime_ns integer, size integer, blocked text not null default '');
            create table links(from_id integer not null, to_path text not null, primary key(from_id, to_path));
            create index links_to_path on links(to_path);
            create virtual table fts using fts5(
                path unindexed, type unindexed, created unindexed, status unindexed,
                title, tags, project, body, tokenize = 'porter unicode61 remove_diacritics 2');
            pragma user_version = {SCHEMA_VERSION}; commit;"))?;
    }
    db.execute_batch("create table if not exists remind_state(session_id text primary key, prompts integer not null, offset integer not null);")?;
    Ok(())
}

pub fn stamp(meta: &fs::Metadata) -> (i64, i64) {
    (
        meta.mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(meta.mtime_nsec()),
        meta.len() as i64,
    )
}

pub fn refresh(root: &Path, db: &mut Connection) -> Result<()> {
    let indexed: HashMap<String, (i64, i64, i64)> = db
        .prepare("select path, id, mtime_ns, size from files")?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?
        .collect::<rusqlite::Result<_>>()?;
    let mut on_disk = HashMap::new();
    for path in pages::markdown_files(&root.join("pages"))? {
        on_disk.insert(
            path.strip_prefix(root)?.to_string_lossy().into_owned(),
            stamp(&path.metadata()?),
        );
    }
    let tx = db.transaction()?;
    for (rel, (id, _, _)) in &indexed {
        if !on_disk.contains_key(rel) {
            tx.execute("delete from links where from_id = ?", [id])?;
            tx.execute("delete from fts where rowid = ?", [id])?;
            tx.execute("delete from files where id = ?", [id])?;
        }
    }
    for (rel, (mtime, size)) in on_disk {
        let old = indexed.get(&rel);
        if old.is_some_and(|(_, t, s)| (*t, *s) == (mtime, size)) {
            continue;
        }
        let text = pages::read(&root.join(&rel))?;
        let page = pages::parse(&text, &rel);
        let findings = scan::scan(&text);
        let blocked = findings.first().map_or("", |f| f.category);
        let id = if let Some((id, _, _)) = old {
            tx.execute("delete from links where from_id = ?", [id])?;
            tx.execute("delete from fts where rowid = ?", [id])?;
            tx.execute(
                "update files set mtime_ns = ?, size = ?, blocked = ? where id = ?",
                params![mtime, size, blocked, id],
            )?;
            *id
        } else {
            tx.execute(
                "insert into files(path, mtime_ns, size, blocked) values (?, ?, ?, ?)",
                params![rel, mtime, size, blocked],
            )?;
            tx.last_insert_rowid()
        };
        tx.execute("insert into fts(rowid,path,type,created,status,title,tags,project,body) values (?,?,?,?,?,?,?,?,?)",
            params![id, rel, page.kind, page.created, page.status, page.title, page.tags, page.project, page.body])?;
        for target in page.links {
            tx.execute(
                "insert into links(from_id, to_path) values (?, ?)",
                params![id, target],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug)]
pub struct Row {
    pub path: String,
    pub kind: String,
    pub status: String,
    pub title: String,
    pub project: String,
    pub tags: String,
    pub mtime: i64,
    pub blocked: String,
    pub backlinks: i64,
}
impl Row {
    pub fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            path: r.get(0)?,
            kind: r.get(1)?,
            status: r.get(2)?,
            title: r.get(3)?,
            project: r.get(4)?,
            tags: r.get(5)?,
            mtime: r.get(6)?,
            blocked: r.get(7)?,
            backlinks: r.get(8)?,
        })
    }
    pub fn blocked_line(&self, path: &str) -> String {
        format!("{path}: [BLOCKED: {}; fix the page]", self.blocked)
    }
}

pub fn linked_from(db: &Connection, path: &str) -> Result<Vec<String>> {
    Ok(db
        .prepare("select files.path from links join files on files.id = links.from_id where links.to_path = ? order by files.path")?
        .query_map([path], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backlink_counts_include_only_links_to_the_page() -> Result<()> {
        let db = Connection::open_in_memory()?;
        initialize(&db)?;
        for (id, name) in [(1, "target"), (2, "source"), (3, "other")] {
            let path = format!("pages/knowledge/{name}.md");
            db.execute(
                "insert into files(id, path, mtime_ns, size) values (?, ?, 0, 0)",
                params![id, path],
            )?;
            db.execute("insert into fts(rowid, path, type, status, title, project, tags) values (?, ?, 'knowledge', '', ?, '', '')", params![id, path, name])?;
        }
        db.execute_batch("insert into links values (2, 'pages/knowledge/target.md'), (3, 'pages/knowledge/target.md'), (2, 'pages/knowledge/missing.md');")?;
        let rows = db
            .prepare(&format!("select {COLUMNS} {JOIN} order by files.id"))?
            .query_map([], Row::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        assert_eq!(
            rows.iter().map(|r| r.backlinks).collect::<Vec<_>>(),
            [2, 0, 0]
        );
        assert_eq!(
            linked_from(&db, "pages/knowledge/target.md")?,
            ["pages/knowledge/other.md", "pages/knowledge/source.md"]
        );
        assert!(db
            .execute(
                "insert into links values (2, 'pages/knowledge/target.md')",
                []
            )
            .is_err());
        Ok(())
    }

    #[test]
    fn bundled_fts5_and_schema_migration() -> Result<()> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("create table files(id integer primary key); pragma user_version = 3;")?;
        initialize(&db)?;
        assert_eq!(
            db.query_row("pragma user_version", [], |r| r.get::<_, i64>(0))?,
            SCHEMA_VERSION
        );
        db.execute(
            "insert into fts(title, body) values ('café blocking', 'replica')",
            [],
        )?;
        assert_eq!(
            db.query_row(
                "select count(*) from fts where fts match 'cafe OR blocked'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            1
        );
        db.execute("insert into remind_state values ('test', 3, 10)", [])?;
        initialize(&db)?;
        assert_eq!(
            db.query_row("select prompts from remind_state", [], |r| r
                .get::<_, i64>(0))?,
            3
        );
        Ok(())
    }
}

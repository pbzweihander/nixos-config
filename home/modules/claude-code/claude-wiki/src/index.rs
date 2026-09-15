use crate::{pages, scan};
use anyhow::Result;
use rusqlite::{params, Connection};
use std::{collections::HashMap, fs, os::unix::fs::MetadataExt, path::Path, time::Duration};

pub const COLUMNS: &str = "fts.path, fts.type, fts.status, fts.title, fts.project, fts.tags, files.mtime_ns, files.blocked";
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
    if db.query_row("pragma user_version", [], |r| r.get::<_, i64>(0))? != 3 {
        db.execute_batch("begin;
            drop table if exists files; drop table if exists fts;
            create table files(id integer primary key, path text unique, mtime_ns integer, size integer, blocked text not null default '');
            create virtual table fts using fts5(
                path unindexed, type unindexed, created unindexed, status unindexed,
                title, tags, project, body, tokenize = 'porter unicode61 remove_diacritics 2');
            pragma user_version = 3; commit;")?;
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
        })
    }
    pub fn blocked_line(&self, path: &str) -> String {
        format!("{path}: [BLOCKED: {}; fix the page]", self.blocked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_fts5_and_schema_migration() -> Result<()> {
        let db = Connection::open_in_memory()?;
        db.execute_batch("create table files(id integer primary key); pragma user_version = 2;")?;
        initialize(&db)?;
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

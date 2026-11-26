use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Nestable transactions let harness staging include helper updates atomically.
pub struct Transaction<'a> {
    db: &'a Connection,
    name: String,
    committed: bool,
}
impl std::ops::Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.db
    }
}
impl Transaction<'_> {
    pub fn commit(mut self) -> Result<()> {
        self.db.execute_batch(&format!("release {}", self.name))?;
        self.committed = true;
        Ok(())
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self
                .db
                .execute_batch(&format!("rollback to {}; release {}", self.name, self.name));
        }
    }
}
pub fn transaction(db: &Connection) -> Result<Transaction<'_>> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = format!(
        "hook_{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    db.execute_batch(&format!("savepoint {name}"))?;
    Ok(Transaction {
        db,
        name,
        committed: false,
    })
}

fn open_direct(dir: &Path) -> Result<Connection> {
    let db = crate::sessions::private_db(&dir.join("state.sqlite"))?;
    db.busy_timeout(std::time::Duration::from_millis(50))?;
    db.pragma_update(None, "journal_mode", "wal")?;
    initialize(&db)?;
    db.execute_batch("create table if not exists state_migrations(name text primary key)")?;
    Ok(db)
}

fn index_marker(index: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(index)?;
    Ok(format!("index-hook-state:{}:{}", meta.dev(), meta.ino()))
}

pub fn mark_index(db: &Connection, index: &Path) -> Result<()> {
    db.execute(
        "insert or ignore into state_migrations values (?)",
        [index_marker(index)?],
    )?;
    Ok(())
}

pub fn open(root: &Path) -> Result<Connection> {
    let dir = crate::storage::prepare(root)?;
    let db = open_direct(&dir)?;
    let index = dir.join("index.sqlite");
    if std::fs::symlink_metadata(&index).is_ok() {
        let marker = index_marker(&index)?;
        let migrated: bool = db.query_row(
            "select exists(select 1 from state_migrations where name=?)",
            [&marker],
            |r| r.get(0),
        )?;
        if !migrated {
            attach_and_migrate(&db, &index, true)?;
            mark_index(&db, &index)?;
        }
    }
    Ok(db)
}

/// Storage migration calls this directly to avoid recursively acquiring its lock.
pub fn import_index(dir: &Path, index: &Path) -> Result<()> {
    let db = open_direct(dir)?;
    attach_and_migrate(&db, index, false)
}

fn attach_and_migrate(db: &Connection, index: &Path, remove: bool) -> Result<()> {
    drop(crate::sessions::private_db(index)?);
    db.execute(
        "attach database ? as pages_index",
        [index.to_string_lossy().as_ref()],
    )?;
    migrate(db, remove)?;
    db.execute_batch("detach database pages_index")?;
    Ok(())
}

pub fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("create table if not exists remind_state(session_id text primary key, prompts integer not null, offset integer not null);
        create table if not exists recall_seen(session_id text, path text, primary key(session_id, path));
        create table if not exists recall_state(session_id text primary key, offset integer not null);")?;
    Ok(())
}

fn migrate(db: &Connection, remove: bool) -> Result<()> {
    let tables = db.prepare("select name, sql from pages_index.sqlite_master where type='table' and (name like 'remind_%' or name like 'recall_%' or name like 'antigravity_%' or name like 'codex_recall_%')")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let tx = db.unchecked_transaction()?;
    for (name, sql) in tables {
        anyhow::ensure!(
            name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "invalid legacy hook table name"
        );
        let exists: bool = tx.query_row(
            "select exists(select 1 from main.sqlite_master where name=?)",
            [&name],
            |r| r.get(0),
        )?;
        if !exists {
            tx.execute_batch(&sql)?;
        }
        tx.execute_batch(&format!(
            "insert or ignore into main.\"{name}\" select * from pages_index.\"{name}\";"
        ))?;
    }
    tx.commit()?;
    if !remove {
        return Ok(());
    }
    // Leave no hook table in the rebuildable database. Copy commits first, so a
    // crash between the two phases can only cause an idempotent replay.
    let names = db.prepare("select name from pages_index.sqlite_master where type='table' and (name like 'remind_%' or name like 'recall_%' or name like 'antigravity_%' or name like 'codex_recall_%')")?
        .query_map([], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for name in names {
        db.execute_batch(&format!("drop table pages_index.\"{name}\""))?;
    }
    Ok(())
}

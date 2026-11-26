//! Private runtime files, isolated from the Markdown repository.
use anyhow::{Result, bail};
use rusqlite::Connection;
use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

pub fn dir(root: &Path) -> PathBuf {
    if let Some(path) = crate::compat::var_os("AGENT_WIKI_STATE_DIR") {
        return crate::expand_home(Path::new(&path));
    }
    let home_data = crate::home().join(".local/share");
    let data = crate::env_path("XDG_DATA_HOME").unwrap_or_else(|| home_data.clone());
    let default = crate::compat::directory(&data);
    let explicit_state = crate::env_path("XDG_STATE_HOME");
    // A custom data home without a paired state home is a custom fixture/root,
    // not permission to put its derived data into the user's live state tree.
    if canonical(root) == canonical(&default)
        && (canonical(&data) == canonical(&home_data) || explicit_state.is_some())
    {
        crate::compat::directory(
            &explicit_state.unwrap_or_else(|| crate::home().join(".local/state")),
        )
    } else {
        let root = canonical(root);
        let mut name = root.file_name().unwrap_or_default().to_os_string();
        name.push("-state");
        root.with_file_name(name)
    }
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .expect("current directory")
                .join(path)
        }
    })
}

fn sync_file(path: &Path) -> Result<()> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(crate::sessions::PRIVATE_OPEN_FLAGS)
        .open(path)?
        .sync_all()?;
    Ok(())
}

fn staging_files(dir: &Path, names: &[&str]) -> Result<Vec<PathBuf>> {
    Ok(fs::read_dir(dir)?
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|entry| {
            names.iter().any(|name| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!(".{name}.migrating-"))
            })
        })
        .map(|entry| entry.path())
        .collect())
}

fn directory(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        bail!("refusing non-directory state path: {}", path.display());
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(crate::sessions::PRIVATE_OPEN_FLAGS)
        .open(path)?;
    if !file.metadata()?.is_dir() {
        bail!("state path is not a directory");
    }
    file.set_permissions(fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn lock(root: &Path, name: &str) -> Result<fd_lock::RwLock<fs::File>> {
    let dir = dir(root);
    directory(&dir)?;
    let path = dir.join(name);
    crate::sessions::repair_private_file(&path, true)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(crate::sessions::PRIVATE_OPEN_FLAGS)
        .open(path)?;
    if !file.metadata()?.is_file() {
        bail!("lock is not a regular file");
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(fd_lock::RwLock::new(file))
}

pub fn prepare(root: &Path) -> Result<PathBuf> {
    let dir = dir(root);
    directory(&dir)?;
    let names = [
        "index.sqlite",
        "state.sqlite",
        "usage.sqlite",
        "sessions.sqlite",
        "records.sqlite",
    ];
    let caches: Vec<_> = match fs::read_dir(root) {
        Ok(entries) => Some(entries),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            None
        }
        Err(e) => return Err(e.into()),
    }
    .into_iter()
    .flatten()
    .filter_map(|entry| entry.ok())
    .filter(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("vectors-") && name.ends_with(".npz"))
    })
    .map(|entry| entry.path())
    .collect();
    let legacy_files: Vec<_> = names
        .iter()
        .flat_map(|name| {
            ["", "-wal", "-shm", "-journal"].map(|suffix| root.join(format!("{name}{suffix}")))
        })
        .chain(std::iter::once(root.join(".lock")))
        .chain(caches.iter().cloned())
        .collect();
    if !legacy_files
        .iter()
        .any(|path| fs::symlink_metadata(path).is_ok())
        && staging_files(&dir, &names)?.is_empty()
    {
        return Ok(dir);
    }
    let mut migration = lock(root, "migration.lock")?;
    let _guard = migration.try_write()?;
    // Validate before cleanup, including stale journals from a killed VACUUM.
    let stale = staging_files(&dir, &names)?;
    for path in legacy_files.iter().chain(&stale) {
        crate::sessions::repair_private_file(path, false)?;
    }
    for path in stale {
        fs::remove_file(path)?;
    }
    sync_file(&dir)?;
    // Serialize against an old binary while its legacy database is snapshotted.
    let legacy = root.join(".lock");
    let mut old_lock = if fs::symlink_metadata(&legacy).is_ok() {
        crate::sessions::repair_private_file(&legacy, false)?;
        Some(fd_lock::RwLock::new(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(crate::sessions::PRIVATE_OPEN_FLAGS)
                .open(&legacy)?,
        ))
    } else {
        None
    };
    let had_lock = old_lock.is_some();
    let _old_guard = old_lock.as_mut().map(|lock| lock.try_write()).transpose()?;
    for name in names {
        let source = root.join(name);
        if fs::symlink_metadata(&source).is_err() {
            for suffix in ["-wal", "-shm", "-journal"] {
                let orphan = root.join(format!("{name}{suffix}"));
                if fs::symlink_metadata(&orphan).is_ok() {
                    if name != "index.sqlite" {
                        bail!("orphaned persistent database sidecar: {name}{suffix}");
                    }
                    fs::remove_file(orphan)?;
                }
            }
            continue;
        }
        let authoritative = dir.join(name);
        let conflict = fs::symlink_metadata(&authoritative).is_ok();
        crate::sessions::repair_private_file(&authoritative, false)?;
        let target = if conflict {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            dir.join(format!("{name}.legacy-{timestamp}"))
        } else {
            authoritative
        };
        let temporary = dir.join(format!(".{name}.migrating-{}", std::process::id()));
        let db = crate::sessions::private_db(&source)?;
        db.busy_timeout(Duration::from_millis(50))?;
        // Prevent an already running legacy writer from changing the source
        // between the snapshot and unlink. VACUUM INTO itself is read-only on
        // the source and runs on a separate connection under this reservation.
        db.execute_batch("begin immediate")?;
        let snapshot = crate::sessions::private_db(&source)?;
        snapshot.busy_timeout(Duration::from_millis(50))?;
        for suffix in ["-wal", "-shm", "-journal"] {
            crate::sessions::repair_private_file(&dir.join(format!("{name}{suffix}")), false)?;
        }
        // Never expose a partial final database: publish only a synced snapshot.
        crate::sessions::repair_private_file(&temporary, true)?;
        snapshot.execute("vacuum into ?", [temporary.to_string_lossy().as_ref()])?;
        sync_file(&temporary)?;
        fs::rename(&temporary, &target)?;
        sync_file(&dir)?;
        if name == "index.sqlite" {
            // Recheck every legacy index, even if an earlier migration marker
            // exists. Existing state rows win; the archive retains all originals.
            crate::state::import_index(&dir, &target)?;
        }
        if conflict {
            eprintln!(
                "agent-wiki: kept state {name}; archived legacy database as {}",
                target.file_name().unwrap().to_string_lossy()
            );
        }
        drop(snapshot);
        for suffix in ["-wal", "-shm", "-journal", ""] {
            let path = root.join(format!("{name}{suffix}"));
            crate::sessions::repair_private_file(&path, false)?;
            match fs::remove_file(path) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        drop(db);
    }
    // Embedding snapshots are derived and can be discarded safely; the daemon
    // rebuilds them beside the migrated index rather than polluting the repo.
    for cache in caches {
        fs::remove_file(cache)?;
    }
    if had_lock {
        fs::remove_file(legacy)?;
    }
    sync_file(&dir)?;
    if root.is_dir() {
        sync_file(root)?;
    }
    Ok(dir)
}

pub fn open(root: &Path, name: &str) -> Result<Connection> {
    let db = crate::sessions::private_db(&prepare(root)?.join(name))?;
    db.busy_timeout(Duration::from_millis(50))?;
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_is_private_and_outside_markdown_root() -> Result<()> {
        let wiki = crate::usage::tests::Wiki::new()?;
        let state = dir(&wiki.root);
        assert_ne!(state, wiki.root);
        assert!(!state.starts_with(&wiki.root));
        for entry in fs::read_dir(&wiki.root)? {
            assert!(
                ["pages", ".gitignore", ".git"].contains(&entry?.file_name().to_str().unwrap())
            );
        }
        assert_eq!(fs::metadata(&state)?.permissions().mode() & 0o777, 0o700);
        for name in ["state.sqlite", "index.sqlite"] {
            assert_eq!(
                fs::metadata(state.join(name))?.permissions().mode() & 0o777,
                0o600
            );
        }
        for name in ["wiki.lock", "sessions.lock", "usage.lock"] {
            let _lock = lock(&wiki.root, name)?;
            assert_eq!(
                fs::metadata(state.join(name))?.permissions().mode() & 0o777,
                0o600
            );
        }
        wiki.index
            .execute("insert into remind_state values ('persist',3,0)", [])?;
        wiki.index
            .execute_batch("pragma pages_index.user_version=0")?;
        let rebuilt = crate::index::open(&wiki.root)?;
        assert_eq!(
            rebuilt.query_row(
                "select prompts from remind_state where session_id='persist'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            3
        );
        let page_db = Connection::open(state.join("index.sqlite"))?;
        assert_eq!(
            page_db.query_row(
                "select count(*) from sqlite_master where name='remind_state'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }
    #[test]
    fn legacy_wal_and_hook_rows_survive_migration() -> Result<()> {
        let holder = crate::usage::tests::Wiki::new()?;
        let root = dir(&holder.root).join("legacy");
        fs::create_dir(&root)?;
        let legacy = Connection::open(root.join("index.sqlite"))?;
        legacy.execute_batch("pragma journal_mode=wal; create table remind_state(session_id text primary key,prompts integer not null,offset integer not null); insert into remind_state values ('old',7,9)")?;
        let mut sources = Vec::new();
        for name in ["usage.sqlite", "sessions.sqlite", "records.sqlite"] {
            let source = Connection::open(root.join(name))?;
            source.execute_batch("pragma journal_mode=wal; create table history(value text); insert into history values ('kept')")?;
            sources.push(source);
        }
        fs::write(root.join("vectors-old.npz"), "derived cache")?;
        fs::write(root.join(".lock"), "")?;
        let db = crate::index::open(&root)?;
        assert_eq!(
            db.query_row(
                "select prompts from remind_state where session_id='old'",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            7
        );
        for name in ["usage.sqlite", "sessions.sqlite", "records.sqlite"] {
            let migrated = open(&root, name)?;
            assert_eq!(
                migrated.query_row("select value from history", [], |r| r.get::<_, String>(0))?,
                "kept"
            );
        }
        assert_eq!(fs::read_dir(&root)?.count(), 0);
        Ok(())
    }
    #[test]
    fn migration_rejects_symlink_sidecars_but_preserves_conflicting_databases() -> Result<()> {
        use std::os::unix::fs::symlink;
        let holder = crate::usage::tests::Wiki::new()?;
        let root = dir(&holder.root).join("unsafe");
        fs::create_dir(&root)?;
        let target = root.join("unrelated");
        fs::write(&target, "unchanged")?;
        Connection::open(root.join("usage.sqlite"))?.execute_batch("create table data(x)")?;
        symlink(&target, root.join("usage.sqlite-wal"))?;
        assert!(prepare(&root).is_err());
        assert_eq!(fs::read_to_string(&target)?, "unchanged");
        fs::remove_file(root.join("usage.sqlite-wal"))?;
        directory(&dir(&root))?;
        fs::write(dir(&root).join("usage.sqlite"), "destination")?;
        prepare(&root)?;
        assert!(!root.join("usage.sqlite").exists());
        assert_eq!(
            fs::read_to_string(dir(&root).join("usage.sqlite"))?,
            "destination"
        );
        let archive = fs::read_dir(dir(&root))?
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("usage.sqlite.legacy-")
            })
            .unwrap();
        let archived = crate::sessions::private_db(&archive.path())?;
        archived.execute("insert into data values (1)", [])?;
        Ok(())
    }
    #[test]
    fn recreated_legacy_index_merges_missing_hook_rows_without_replacing_state() -> Result<()> {
        let wiki = crate::usage::tests::Wiki::new()?;
        wiki.index
            .execute("insert into remind_state values ('kept',9,0)", [])?;
        for iteration in 0..2 {
            let old = Connection::open(wiki.root.join("index.sqlite"))?;
            old.execute_batch("create table remind_state(session_id text primary key,prompts integer not null,offset integer not null); insert into remind_state values ('kept',1,0)")?;
            old.execute(
                "insert into remind_state values (?,3,0)",
                [format!("new-{iteration}")],
            )?;
            drop(old);
            fs::write(wiki.root.join(".lock"), "")?;
            let new = crate::index::open(&wiki.root)?;
            assert_eq!(
                new.query_row(
                    "select prompts from remind_state where session_id='kept'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                9
            );
            assert_eq!(
                new.query_row(
                    "select prompts from remind_state where session_id=?",
                    [format!("new-{iteration}")],
                    |r| r.get::<_, i64>(0)
                )?,
                3
            );
            assert!(!wiki.root.join("index.sqlite").exists());
            assert!(!wiki.root.join(".lock").exists());
        }
        assert_eq!(
            fs::read_dir(dir(&wiki.root))?
                .filter_map(|e| e.ok())
                .filter(|e| e
                    .file_name()
                    .to_string_lossy()
                    .starts_with("index.sqlite.legacy-"))
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn interrupted_snapshot_is_removed_and_legacy_data_recovers() -> Result<()> {
        let wiki = crate::usage::tests::Wiki::new()?;
        let root = dir(&wiki.root).join("crash");
        fs::create_dir(&root)?;
        directory(&dir(&root))?;
        let old = Connection::open(root.join("sessions.sqlite"))?;
        old.execute_batch("create table data(value); insert into data values ('preserved')")?;
        drop(old);
        for suffix in ["", "-journal"] {
            fs::write(
                dir(&root).join(format!(".sessions.sqlite.migrating-123{suffix}")),
                "partial",
            )?;
        }
        prepare(&root)?;
        assert!(staging_files(&dir(&root), &["sessions.sqlite"])?.is_empty());
        assert_eq!(
            open(&root, "sessions.sqlite")?
                .query_row("select value from data", [], |r| r.get::<_, String>(0))?,
            "preserved"
        );
        assert!(!root.join("sessions.sqlite").exists());
        // A marker from the former one-shot migration must not skip a new index.
        let state = crate::state::open(&root)?;
        state.execute(
            "insert or ignore into state_migrations values ('index-hook-state')",
            [],
        )?;
        let index = Connection::open(dir(&root).join("index.sqlite"))?;
        index.execute_batch("create table recall_seen(session_id text,path text,primary key(session_id,path)); insert into recall_seen values ('restored','page')")?;
        drop(index);
        assert_eq!(
            crate::state::open(&root)?
                .query_row("select count(*) from recall_seen", [], |r| r
                    .get::<_, i64>(0))?,
            1
        );
        Ok(())
    }
}

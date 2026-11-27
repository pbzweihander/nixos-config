//! Read-only database access that never writes to the filesystem.
//!
//! `SQLITE_OPEN_READ_ONLY` alone still creates `-wal` and `-shm` files for a
//! WAL-mode database. A rollback-journal database is opened in place, read-only,
//! so SQLite's file locks keep reads consistent with a concurrent writer. A
//! WAL-mode database is copied into memory together with the committed frames of
//! its `-wal` file, replayed the way SQLite recovers a WAL, and checked to be
//! stable across the copy.
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};

const HEADER: usize = 100;
const WAL_HEADER: usize = 32;
const FRAME_HEADER: usize = 24;
const ATTEMPTS: u32 = 5;

/// Opens `path` without creating or modifying any file, also in a read-only
/// directory. Callers check that `path` exists.
pub fn open(path: &Path) -> Result<Connection> {
    let mut header = [0u8; HEADER];
    let read = read_file(path, &mut header)?;
    // Bytes 18 and 19 are the file format versions: 2 means WAL mode.
    if read < HEADER || header[18] != 2 {
        let db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        // A reader holds no lock between statements; wait out a writer's commit.
        db.busy_timeout(Duration::from_millis(1000))?;
        return Ok(db);
    }
    let image = stable_image(path)?;
    let mut db = Connection::open_in_memory()?;
    db.deserialize_read_exact("main", image.as_slice(), image.len(), true)?;
    Ok(db)
}

/// Snapshot of a WAL-mode database and its committed WAL frames. Retries when
/// the main file or the WAL header changed during the copy (a checkpoint or a
/// WAL restart); appended frames are harmless, as replay stops at the last
/// complete commit that was copied.
fn stable_image(path: &Path) -> Result<Vec<u8>> {
    let mut wal_path = path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal_path = Path::new(&wal_path);
    for _ in 0..ATTEMPTS {
        let before = Stamp::of(path, wal_path)?;
        let main = read_all(path)?;
        let wal = match read_all(wal_path) {
            Ok(wal) => Some(wal),
            Err(e) if not_found(&e) => None,
            Err(e) => return Err(e),
        };
        if before == Stamp::of(path, wal_path)? {
            return replay(main, wal.as_deref());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("{} kept changing while it was read; retry", path.display())
}

#[derive(PartialEq)]
struct Stamp {
    main: (u64, u64, i64, i64),
    wal: Option<(u64, [u8; WAL_HEADER])>,
}

impl Stamp {
    fn of(main: &Path, wal: &Path) -> Result<Self> {
        let meta = fs::symlink_metadata(main)?;
        let mut header = [0u8; WAL_HEADER];
        let wal = match fs::symlink_metadata(wal) {
            Ok(meta) => {
                read_file(wal, &mut header)?;
                Some((meta.ino(), header))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            main: (meta.ino(), meta.len(), meta.mtime(), meta.mtime_nsec()),
            wal,
        })
    }
}

fn not_found(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

fn open_file(path: &Path) -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(crate::sessions::PRIVATE_OPEN_FLAGS)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "refusing non-regular database file: {}",
        path.display()
    );
    Ok(file)
}

fn read_file(path: &Path, buffer: &mut [u8]) -> Result<usize> {
    let mut file = open_file(path)?;
    let mut read = 0;
    while read < buffer.len() {
        match file.read(&mut buffer[read..])? {
            0 => break,
            n => read += n,
        }
    }
    Ok(read)
}

fn read_all(path: &Path) -> Result<Vec<u8>> {
    let mut data = vec![];
    open_file(path)?.read_to_end(&mut data)?;
    Ok(data)
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes[..4].try_into().expect("four bytes"))
}

/// SQLite's WAL checksum over 32-bit words in the byte order the magic names.
fn checksum(big_endian: bool, data: &[u8], mut sum: (u32, u32)) -> (u32, u32) {
    for words in data.chunks_exact(8) {
        let word = |bytes: &[u8]| {
            let bytes = bytes.try_into().expect("four bytes");
            if big_endian {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            }
        };
        sum.0 = sum.0.wrapping_add(word(&words[..4])).wrapping_add(sum.1);
        sum.1 = sum.1.wrapping_add(word(&words[4..])).wrapping_add(sum.0);
    }
    sum
}

/// Applies every frame up to the last valid commit, then marks the image as a
/// rollback-journal database so the in-memory copy needs no WAL.
fn replay(mut image: Vec<u8>, wal: Option<&[u8]>) -> Result<Vec<u8>> {
    if let Some(wal) = wal.filter(|wal| wal.len() >= WAL_HEADER) {
        let big_endian = match be32(wal) {
            0x377f_0682 => Some(false),
            0x377f_0683 => Some(true),
            _ => None,
        };
        let header_sum = big_endian.map(|be| checksum(be, &wal[..24], (0, 0)));
        if let (Some(big_endian), Some(sum)) = (big_endian, header_sum)
            && sum == (be32(&wal[24..]), be32(&wal[28..]))
        {
            apply_frames(&mut image, wal, big_endian, sum)?;
        }
    }
    ensure!(image.len() >= HEADER, "database image is too short");
    image[18] = 1;
    image[19] = 1;
    Ok(image)
}

fn apply_frames(
    image: &mut Vec<u8>,
    wal: &[u8],
    big_endian: bool,
    mut sum: (u32, u32),
) -> Result<()> {
    let page_size = match be32(&wal[8..]) {
        1 => 65536,
        n => usize::try_from(n)?,
    };
    ensure!(
        page_size.is_power_of_two() && (512..=65536).contains(&page_size),
        "invalid WAL page size {page_size}"
    );
    let salt = &wal[16..24];
    let mut pending: Vec<(usize, &[u8])> = vec![];
    let mut offset = WAL_HEADER;
    while let Some(frame) = wal.get(offset..offset + FRAME_HEADER + page_size) {
        offset += FRAME_HEADER + page_size;
        let (header, page) = frame.split_at(FRAME_HEADER);
        if &header[8..16] != salt {
            break;
        }
        sum = checksum(big_endian, page, checksum(big_endian, &header[..8], sum));
        if sum != (be32(&header[16..]), be32(&header[20..])) {
            break;
        }
        let number = usize::try_from(be32(header))?;
        ensure!(number > 0, "invalid WAL page number");
        pending.push((number, page));
        let pages = usize::try_from(be32(&header[4..]))?;
        if pages > 0 {
            for (number, page) in pending.drain(..) {
                let start = (number - 1) * page_size;
                if image.len() < start + page_size {
                    image.resize(start + page_size, 0);
                }
                image[start..start + page_size].copy_from_slice(page);
            }
            image.resize(pages * page_size, 0);
        }
    }
    Ok(())
}

/// Opens the index read-only, checking its schema version.
pub fn open_index(path: &Path, version: i64) -> Result<Connection> {
    let db = open(path).with_context(|| format!("reading {}", path.display()))?;
    let found: i64 = db.query_row("pragma user_version", [], |r| r.get(0))?;
    ensure!(
        found == version,
        "index schema {found} is not {version}; run the command without --no-refresh to rebuild it"
    );
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn count(db: &Connection) -> i64 {
        db.query_row("select count(*), sum(length(v)) from t", [], |r| {
            Ok(r.get::<_, i64>(0)? * 1_000_000 + r.get::<_, Option<i64>>(1)?.unwrap_or(0))
        })
        .unwrap()
    }

    #[test]
    fn wal_frames_are_replayed_in_memory_without_new_files() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = Dir(std::env::temp_dir().join(format!("wiki-snapshot-{}", std::process::id())));
        fs::create_dir(&dir.0)?;
        let path = dir.0.join("db.sqlite");
        let writer = Connection::open(&path)?;
        writer.execute_batch(
            "pragma journal_mode=wal; pragma wal_autocheckpoint=0;
             create table t(v text); insert into t values ('checkpointed');
             pragma wal_checkpoint(truncate);",
        )?;
        // Committed but not checkpointed: these rows exist only in the -wal.
        for i in 0..200 {
            writer.execute(
                "insert into t values (?)",
                [format!("wal-{i}-{}", "x".repeat(i))],
            )?;
        }
        writer.execute_batch("pragma user_version=7")?;
        // An open transaction must stay invisible.
        writer.execute_batch("begin; insert into t values ('uncommitted');")?;
        let expected = {
            let reader = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            count(&reader)
        };
        let before = entries(&dir.0);
        assert!(before.contains(&"db.sqlite-wal".to_owned()));
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o500))?;
        let snapshot = open_index(&path, 7);
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o700))?;
        assert_eq!(count(&snapshot?), expected);
        assert_eq!(entries(&dir.0), before);
        writer.execute_batch("rollback")?;
        drop(writer);
        // Clean WAL-mode database without sidecars.
        assert_eq!(entries(&dir.0), ["db.sqlite"]);
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o500))?;
        let clean = open(&path);
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o700))?;
        assert_eq!(count(&clean?), expected);
        assert_eq!(entries(&dir.0), ["db.sqlite"]);
        Ok(())
    }

    #[test]
    fn torn_or_foreign_frames_are_ignored() -> Result<()> {
        let dir =
            Dir(std::env::temp_dir().join(format!("wiki-snapshot-torn-{}", std::process::id())));
        fs::create_dir(&dir.0)?;
        let path = dir.0.join("db.sqlite");
        let writer = Connection::open(&path)?;
        writer.execute_batch(
            "pragma journal_mode=wal; pragma wal_autocheckpoint=0;
             create table t(v text); pragma wal_checkpoint(truncate);
             insert into t values ('first');",
        )?;
        let main = fs::read(&path)?;
        let wal = fs::read(dir.0.join("db.sqlite-wal"))?;
        writer.execute("insert into t values ('second')", [])?;
        let longer = fs::read(dir.0.join("db.sqlite-wal"))?;
        drop(writer);
        let load = |wal: &[u8]| -> Result<i64> {
            let image = replay(main.clone(), Some(wal))?;
            let mut db = Connection::open_in_memory()?;
            db.deserialize_read_exact("main", image.as_slice(), image.len(), true)?;
            Ok(db.query_row("select count(*) from t", [], |r| r.get(0))?)
        };
        assert_eq!(load(&wal)?, 1);
        assert_eq!(load(&longer)?, 2);
        // A torn last frame drops only the commit it belongs to.
        assert_eq!(load(&longer[..longer.len() - 1])?, 1);
        let mut corrupt = longer.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xff;
        assert_eq!(load(&corrupt)?, 1);
        // A WAL with another salt (restarted) contributes nothing.
        let mut foreign = longer;
        foreign[16] ^= 0xff;
        assert_eq!(load(&foreign)?, 0);
        Ok(())
    }
}

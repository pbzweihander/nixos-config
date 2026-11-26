//! Bounded hook input and resumable, replacement-aware JSONL cursors.
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::{Duration, Instant},
};
const CAP: usize = 4 * 1024 * 1024;

pub fn stdin() -> Result<String> {
    let (send, recv) = std::sync::mpsc::sync_channel(64);
    std::thread::spawn(move || {
        let mut input = std::io::stdin().take(64 * 1024);
        let mut chunk = [0u8; 1024];
        while let Ok(n) = input.read(&mut chunk) {
            if n == 0 || send.send(chunk[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut bytes = Vec::new();
    while bytes.len() < 64 * 1024 {
        let Ok(chunk) = recv.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        else {
            break;
        };
        bytes.extend(chunk);
    }
    Ok(String::from_utf8(bytes)?)
}

pub fn lines(
    db: &Connection,
    key: &str,
    path: Option<&str>,
    legacy: u64,
    mut visit: impl FnMut(&[u8]) -> Result<()>,
) -> Result<(u64, bool)> {
    let Some(path) = path else {
        return Ok((legacy, true));
    };
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(crate::sessions::PRIVATE_OPEN_FLAGS)
        .open(Path::new(path))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((legacy, true)),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        file.metadata()?.file_type().is_file(),
        "hook transcript must be a regular file"
    );
    db.execute_batch("create table if not exists hook_cursors(key text primary key, path text not null, offset integer not null, fingerprint text not null, discard integer not null)")?;
    let old: Option<(String, u64, String, bool)> = db
        .query_row(
            "select path,offset,fingerprint,discard from hook_cursors where key=?",
            [key],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (mut offset, mut discard) = match old {
        Some((old_path, at, hash, discard))
            if old_path == path
                && file.metadata()?.len() >= at
                && crate::usage::fingerprint(&mut file, at)? == hash =>
        {
            (at, discard)
        }
        _ => (0, false),
    };
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut remaining = CAP;
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut line = Vec::new();
    let mut complete = false;
    while remaining > 0 && Instant::now() < deadline {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            complete = true;
            break;
        }
        let available = buf.len().min(remaining);
        let n = buf[..available]
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available, |n| n + 1);
        let ended = buf[n - 1] == b'\n';
        if !discard {
            line.extend_from_slice(&buf[..n]);
        }
        reader.consume(n);
        remaining -= n;
        if discard {
            offset += n as u64;
            if ended {
                discard = false;
            }
        } else if ended {
            visit(&line)?;
            offset += line.len() as u64;
            line.clear();
        } else if line.len() == CAP {
            offset += line.len() as u64;
            line.clear();
            discard = true;
        }
    }
    // Only complete lines (or explicitly discarded oversized lines) advance.
    if !line.is_empty() {
        complete = false;
    }
    let mut file: File = reader.into_inner();
    let hash = crate::usage::fingerprint(&mut file, offset)?;
    db.execute("insert into hook_cursors values (?,?,?,?,?) on conflict(key) do update set path=excluded.path,offset=excluded.offset,fingerprint=excluded.fingerprint,discard=excluded.discard",params![key,path,i64::try_from(offset)?,hash,discard])?;
    Ok((offset, complete))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_lines_progress_and_replacements_restart() -> Result<()> {
        let wiki = crate::usage::tests::Wiki::new()?;
        let path = crate::storage::dir(&wiki.root).join("transcript.jsonl");
        let path = path.to_str().unwrap();
        let mut text = vec![b'x'; CAP + 100];
        text.extend_from_slice(b"\n{}\n");
        std::fs::write(path, &text)?;
        let mut visits = 0;
        let (first, _) = lines(&wiki.index, "test", Some(path), 0, |_| {
            visits += 1;
            Ok(())
        })?;
        assert_eq!(first, CAP as u64);
        assert_eq!(visits, 0);
        let (end, _) = lines(&wiki.index, "test", Some(path), first, |_| {
            visits += 1;
            Ok(())
        })?;
        assert_eq!(end, text.len() as u64);
        assert_eq!(visits, 1);
        text[0] = b'y';
        text.extend_from_slice(b"[]\n");
        std::fs::write(path, &text)?;
        let (reset, _) = lines(&wiki.index, "test", Some(path), end, |_| Ok(()))?;
        assert_eq!(reset, CAP as u64);
        Ok(())
    }
    #[test]
    fn partial_tail_is_retried_and_nonregular_files_are_rejected() -> Result<()> {
        let wiki = crate::usage::tests::Wiki::new()?;
        let path = crate::storage::dir(&wiki.root).join("tail.jsonl");
        std::fs::write(&path, b"{}\n{")?;
        assert_eq!(
            lines(&wiki.index, "tail", path.to_str(), 0, |_| Ok(()))?.0,
            3
        );
        std::fs::write(&path, b"{}\n{}\n")?;
        let mut calls = 0;
        assert_eq!(
            lines(&wiki.index, "tail", path.to_str(), 3, |_| {
                calls += 1;
                Ok(())
            })?
            .0,
            6
        );
        assert_eq!(calls, 1);
        assert!(lines(&wiki.index, "dir", wiki.root.to_str(), 0, |_| Ok(())).is_err());
        Ok(())
    }
}

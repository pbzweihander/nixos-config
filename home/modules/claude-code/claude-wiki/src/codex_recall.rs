//! Bound unknown-exit-status recall without persisting command diagnostics.
use anyhow::Result;
use regex::Regex;
use rusqlite::{Connection, params};
use std::{
    collections::{HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::LazyLock,
};

const SESSION_LIMIT: i64 = 3;

fn key(line: &str) -> String {
    static NUMBERS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b(?:0x[0-9a-f]+|[0-9]+)\b").unwrap());
    let normalized = line
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut hash = DefaultHasher::new();
    NUMBERS.replace_all(&normalized, "#").hash(&mut hash);
    // This is a non-security deduplication key, not persisted raw error text.
    format!("{:016x}", hash.finish())
}

pub fn pending(
    db: &Connection,
    session: &str,
    text: &str,
    explain: bool,
) -> Result<Vec<(String, String)>> {
    if session.is_empty() && !explain {
        return Ok(vec![]);
    }
    let mut seen = HashSet::new();
    if !explain {
        db.execute_batch(
            "create table if not exists codex_recall_lines(
                session_id text, line_key text, primary key(session_id,line_key));
             create table if not exists codex_recall_budget(
                session_id text primary key, recalls integer not null);",
        )?;
        let used: i64 = db.query_row(
            "select coalesce((select recalls from codex_recall_budget where session_id=?),0)",
            [session],
            |r| r.get(0),
        )?;
        if used >= SESSION_LIMIT {
            return Ok(vec![]);
        }
        seen = db
            .prepare("select line_key from codex_recall_lines where session_id=?")?
            .query_map([session], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok(text
        .lines()
        .filter_map(|line| {
            let key = key(line);
            seen.insert(key.clone()).then(|| (line.to_owned(), key))
        })
        .collect())
}

pub fn record(db: &Connection, session: &str, lines: &[(String, String)]) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    for (_, key) in lines {
        tx.execute(
            "insert or ignore into codex_recall_lines values (?,?)",
            params![session, key],
        )?;
    }
    tx.execute(
        "insert into codex_recall_budget values (?,1)
         on conflict(session_id) do update set recalls=recalls+1",
        [session],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicates_normalized_lines_and_caps_emissions_per_session() -> Result<()> {
        let db = Connection::open_in_memory()?;
        let lines = pending(&db, "one", "error: process 123 failed", false)?;
        assert_eq!(lines.len(), 1);
        record(&db, "one", &lines)?;
        assert!(pending(&db, "one", "ERROR:  process 987 failed", false)?.is_empty());
        for line in ["error: invalid socket state", "error: permission denied"] {
            let lines = pending(&db, "one", line, false)?;
            assert_eq!(lines.len(), 1);
            record(&db, "one", &lines)?;
        }
        assert!(pending(&db, "one", "error: fourth distinct message", false)?.is_empty());
        assert_eq!(
            pending(&db, "two", "error: process 123 failed", false)?.len(),
            1
        );
        assert!(pending(&db, "", "error: unavailable", false)?.is_empty());
        // Explain neither creates tables nor consumes a session budget.
        let fresh = Connection::open_in_memory()?;
        assert_eq!(pending(&fresh, "", "error: unavailable", true)?.len(), 1);
        assert_eq!(
            fresh.query_row("select count(*) from sqlite_master", [], |r| r
                .get::<_, i64>(0))?,
            0
        );
        Ok(())
    }
}

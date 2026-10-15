//! Play time in Pegasus' `stats.db` (docs/pegasus-format.md in the GPD repo, 5): one row per play
//! (start in seconds since the epoch, duration in seconds) under the game file's path. Same schema and
//! statements as Pegasus, so both read each other's history. Steam games are stored as `steam:<appid>`
//! (Pegasus skips paths it doesn't know).

use std::collections::HashMap;
use std::path::Path;

use clamshell_library::Plays;
use rusqlite::{Connection, OptionalExtension};

/// Every path's plays; empty if there is no database yet.
pub fn read(db: &Path) -> HashMap<String, Plays> {
    let mut out: HashMap<String, Plays> = HashMap::new();
    if !db.exists() {
        return out;
    }
    let rows = Connection::open(db).and_then(|c| {
        let mut stmt = c.prepare("SELECT paths.path, plays.start_time, plays.duration FROM plays INNER JOIN paths ON plays.path_id=paths.id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
        rows.collect::<Result<Vec<_>, _>>()
    });
    match rows {
        Ok(rows) => {
            for (path, start, duration) in rows {
                out.entry(path).or_default().add(start.max(0) as u64, duration.max(0) as u64);
            }
        }
        Err(e) => log::warn!("stats {}: {e}", db.display()),
    }
    out
}

/// Adds one play, creating the tables (Pegasus' exact schema) if they are missing.
pub fn record(db: &Path, path: &str, start: u64, duration: u64) -> rusqlite::Result<()> {
    let mut c = Connection::open(db)?;
    let tx = c.transaction()?;
    tx.execute("CREATE TABLE IF NOT EXISTS paths(id INTEGER PRIMARY KEY,path TEXT UNIQUE NOT NULL)", [])?;
    tx.execute(
        "CREATE TABLE IF NOT EXISTS plays(id INTEGER PRIMARY KEY,path_id INTEGER NOT NULL REFERENCES plays(id),start_time INTEGER NOT NULL,duration INTEGER NOT NULL)",
        [],
    )?;
    let id: Option<i64> = tx.query_row("SELECT id FROM paths WHERE path = ?", [path], |r| r.get(0)).optional()?;
    let id = match id {
        Some(id) => id,
        None => {
            tx.execute("INSERT INTO paths VALUES(null, ?)", [path])?;
            tx.last_insert_rowid()
        }
    };
    tx.execute("INSERT INTO plays VALUES(null, ?, ?, ?)", rusqlite::params![id, start as i64, duration as i64])?;
    tx.commit()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_back() {
        let db = std::env::temp_dir().join(format!("clamshell-stats-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        assert!(read(&db).is_empty());
        record(&db, "/g/pc/diablo.sh", 1000, 60).unwrap();
        record(&db, "/g/pc/diablo.sh", 5000, 30).unwrap();
        record(&db, "steam:1245620", 2000, 0).unwrap();
        let plays = read(&db);
        assert_eq!(plays.get("/g/pc/diablo.sh"), Some(&Plays { count: 2, seconds: 90, last: 5030 }));
        assert_eq!(plays.get("steam:1245620").map(|p| p.count), Some(1));
        // Pegasus' schema, byte for byte.
        let c = Connection::open(&db).unwrap();
        let sql: String = c.query_row("SELECT sql FROM sqlite_master WHERE name='plays'", [], |r| r.get(0)).unwrap();
        assert_eq!(sql, "CREATE TABLE plays(id INTEGER PRIMARY KEY,path_id INTEGER NOT NULL REFERENCES plays(id),start_time INTEGER NOT NULL,duration INTEGER NOT NULL)");
        let _ = std::fs::remove_file(&db);
    }
}

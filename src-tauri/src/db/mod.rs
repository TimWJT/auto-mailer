//! SQLite storage. One connection behind a mutex: the app is single-user and
//! every query is short, so this is simpler than a pool and plenty fast.
//! Never hold the lock across an `.await`.

mod schema;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    path: Option<PathBuf>,
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        let conn = Connection::open(path)?;
        let db = Db {
            conn: Arc::new(Mutex::new(conn)),
            path: Some(path.to_path_buf()),
        };
        db.init()?;
        Ok(db)
    }

    pub fn open_in_memory() -> AppResult<Self> {
        let db = Db {
            conn: Arc::new(Mutex::new(Connection::open_in_memory()?)),
            path: None,
        };
        db.init()?;
        Ok(db)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn init(&self) -> AppResult<()> {
        let mut conn = self.lock();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;
             CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
        )?;
        migrate(&mut conn)
    }

    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock would poison it; the data itself is
        // still consistent (SQLite is transactional), so carry on.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the open database with another file's contents (restore).
    /// The source is validated first; on failure the current data is untouched.
    pub fn replace_with(&self, source: &Path) -> AppResult<()> {
        let check = Connection::open(source)
            .map_err(|_| AppError::user("That file isn't an Auto Mailer backup."))?;
        let version: Option<i64> = check
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
            .flatten();
        let version = version.ok_or_else(|| AppError::user("That file isn't an Auto Mailer backup."))?;
        if version as usize > schema::MIGRATIONS.len() {
            return Err(AppError::user(
                "That backup was made by a newer version of Auto Mailer. Update the app first.",
            ));
        }
        drop(check);

        let mut conn = self.lock();
        // Copy page-by-page from the backup into the live connection.
        let src = Connection::open(source)?;
        {
            let backup = rusqlite::backup::Backup::new(&src, &mut conn)?;
            backup.run_to_completion(256, std::time::Duration::from_millis(0), None)?;
        }
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        migrate(&mut conn)
    }

    /// Write a consistent copy of the database to `dest` (backup).
    pub fn backup_to(&self, dest: &Path) -> AppResult<()> {
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        let conn = self.lock();
        conn.execute("VACUUM INTO ?1", params![dest.to_string_lossy()])?;
        Ok(())
    }
}

fn migrate(conn: &mut Connection) -> AppResult<()> {
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |r| r.get(0),
    )?;
    for (i, sql) in schema::MIGRATIONS.iter().enumerate() {
        let version = i as i64 + 1;
        if version <= current {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute("DELETE FROM schema_version", [])?;
        tx.execute("INSERT INTO schema_version (version) VALUES (?1)", [version])?;
        tx.commit()?;
        tracing::info!(version, "applied migration");
    }
    Ok(())
}

// ---- settings -------------------------------------------------------------

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_once_and_are_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, schema::MIGRATIONS.len());
        drop(conn);
        // Running again must be a no-op.
        migrate(&mut db.lock()).unwrap();
    }

    #[test]
    fn backup_and_restore_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let a = Db::open(&dir.path().join("a.sqlite")).unwrap();
        set_setting(&a.lock(), "k", "v1").unwrap();
        let backup = dir.path().join("backup.sqlite");
        a.backup_to(&backup).unwrap();
        set_setting(&a.lock(), "k", "v2").unwrap();
        a.replace_with(&backup).unwrap();
        assert_eq!(get_setting(&a.lock(), "k").unwrap().as_deref(), Some("v1"));
    }

    #[test]
    fn restore_rejects_non_backup_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = Db::open(&dir.path().join("a.sqlite")).unwrap();
        let junk = dir.path().join("junk.sqlite");
        std::fs::write(&junk, b"not a database").unwrap();
        assert!(a.replace_with(&junk).is_err());
    }
}

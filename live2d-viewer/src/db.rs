use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

/// A model record stored in the local history database.
#[allow(dead_code)]
pub struct ModelRecord {
    pub file_path: String,
    pub name: String,
    pub model_version: String, // "V2" or "V3"
    pub zoom_scale: Option<f32>,
    pub last_opened: String, // ISO datetime string from SQLite
}

/// Local SQLite database for the viewer.
///
/// Stores global key–value settings and a model history table.
/// All operations are synchronous and run on the calling thread.
pub struct AppDb {
    conn: Connection,
}

impl AppDb {
    /// Open (or create) the database at `path`, enable WAL mode, and
    /// ensure the schema tables exist.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Enable Write-Ahead Logging for better concurrent-read performance.
        conn.execute_batch("PRAGMA journal_mode=WAL")?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS global_settings (
                key    TEXT PRIMARY KEY,
                value  TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS model_history (
                file_path    TEXT PRIMARY KEY,
                name         TEXT NOT NULL,
                model_version TEXT NOT NULL,
                zoom_scale   REAL,
                last_opened  TEXT NOT NULL DEFAULT (datetime('now')),
                created_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );
            ",
        )?;

        Ok(Self { conn })
    }

    /// Look up a global setting by key. Returns `None` when the key does
    /// not exist.
    #[allow(dead_code)]
    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT value FROM global_settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .ok()
    }

    /// Insert or replace a global setting.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO global_settings (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Insert or update a model history record.
    ///
    /// When a record with the same `file_path` already exists its
    /// `name`, `model_version`, `zoom_scale` and `last_opened` fields
    /// are updated and the row is *not* duplicated.
    pub fn add_or_update_model(
        &self,
        file_path: &str,
        name: &str,
        model_version: &str,
        zoom_scale: Option<f32>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO model_history (file_path, name, model_version, zoom_scale) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(file_path) DO UPDATE SET \
                name=?2, model_version=?3, zoom_scale=?4, last_opened=datetime('now')",
            rusqlite::params![file_path, name, model_version, zoom_scale],
        )?;
        Ok(())
    }

    /// Return every model history record ordered by `last_opened`
    /// descending (most recent first).
    pub fn model_history(&self) -> Result<Vec<ModelRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, name, model_version, zoom_scale, last_opened \
             FROM model_history ORDER BY last_opened DESC",
        )?;

        let rows = stmt.query_map(rusqlite::params![], |row| {
            Ok(ModelRecord {
                file_path: row.get(0)?,
                name: row.get(1)?,
                model_version: row.get(2)?,
                zoom_scale: row.get(3)?,
                last_opened: row.get(4)?,
            })
        })?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    /// Retrieve a single model record by its file path.
    pub fn get_model(&self, file_path: &str) -> Result<Option<ModelRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, name, model_version, zoom_scale, last_opened \
             FROM model_history WHERE file_path = ?1",
        )?;

        let mut rows = stmt.query_map(rusqlite::params![file_path], |row| {
            Ok(ModelRecord {
                file_path: row.get(0)?,
                name: row.get(1)?,
                model_version: row.get(2)?,
                zoom_scale: row.get(3)?,
                last_opened: row.get(4)?,
            })
        })?;

        match rows.next() {
            Some(Ok(record)) => Ok(Some(record)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// Update the zoom scale for a model. Pass `None` to clear the value.
    pub fn set_zoom(&self, file_path: &str, zoom_scale: Option<f32>) -> Result<()> {
        self.conn.execute(
            "UPDATE model_history SET zoom_scale = ?1 WHERE file_path = ?2",
            rusqlite::params![zoom_scale, file_path],
        )?;
        Ok(())
    }

    /// Remove a model from the history (e.g. when its directory no longer
    /// exists on disk).
    pub fn remove_model(&self, file_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM model_history WHERE file_path = ?1",
            rusqlite::params![file_path],
        )?;
        Ok(())
    }

    /// Rename a model in the history (user-friendly display name).
    pub fn rename_model(&self, file_path: &str, new_name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE model_history SET name = ?1 WHERE file_path = ?2",
            rusqlite::params![new_name, file_path],
        )?;
        Ok(())
    }

    /// Given the current model's `file_path`, return the previous and next
    /// model paths in stable `file_path ASC` order (wrapping around).
    /// Returns `(prev, next)` or `None` if only one record exists.
    pub fn prev_next_paths(&self, current: &str) -> Result<Option<(String, String)>> {
        let paths: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT file_path FROM model_history ORDER BY file_path ASC",
            )?;
            let rows = stmt.query_map(rusqlite::params![], |row| row.get::<_, String>(0))?;
            let mut v = Vec::new();
            for r in rows {
                v.push(r?);
            }
            v
        };
        if paths.len() < 2 {
            return Ok(None);
        }
        let idx = paths.iter().position(|p| p == current);
        let idx = match idx {
            Some(i) => i,
            None => return Ok(None),
        };
        let prev = if idx == 0 { paths.len() - 1 } else { idx - 1 };
        let next = (idx + 1) % paths.len();
        Ok(Some((paths[prev].clone(), paths[next].clone())))
    }
}

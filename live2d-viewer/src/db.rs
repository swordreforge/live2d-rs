use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use tokio::runtime::Runtime;

/// Single-threaded runtime used to bridge libsql's async API into the
/// viewer's synchronous event loop.  Initialised once at first DB access.
fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("failed to create tokio runtime for libSQL")
    })
}

/// A model record stored in the local history database.
#[allow(dead_code)]
pub struct ModelRecord {
    pub file_path: String,
    pub name: String,
    pub model_version: String, // "V2" or "V3"
    pub zoom_scale: Option<f32>,
    pub last_opened: String, // ISO datetime string from SQLite
}

/// Local libSQL database for the viewer.
///
/// Stores global key–value settings and a model history table.
/// All operations are synchronous on the caller's thread; async libsql API
/// is bridged via a global tokio runtime.
pub struct AppDb {
    conn: libsql::Connection,
}

impl AppDb {
    /// Open (or create) the database at `path`, enable WAL mode, and
    /// ensure the schema tables exist.
    pub fn open(path: &Path) -> Result<Self> {
        let path_str = path.to_str().context("non-UTF-8 db path")?;
        let db = rt().block_on(libsql::Builder::new_local(path_str).build())?;
        let conn = db.connect()?;

        rt().block_on(conn.execute_batch("PRAGMA journal_mode=WAL"))?;
        rt().block_on(conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS global_settings (
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
            );",
        ))?;

        Ok(Self { conn })
    }

    /// Look up a global setting by key. Returns `None` when the key does
    /// not exist.
    #[allow(dead_code)]
    pub fn get_setting(&self, key: &str) -> Option<String> {
        let mut rows = rt()
            .block_on(self.conn.query(
                "SELECT value FROM global_settings WHERE key = ?1",
                libsql::params![key],
            ))
            .ok()?;
        let row = rt().block_on(rows.next()).ok()??;
        row.get::<String>(0).ok()
    }

    /// Insert or replace a global setting.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        rt().block_on(self.conn.execute(
            "INSERT INTO global_settings (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            libsql::params![key, value],
        ))?;
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
        // libSQL ToValue supports Option<T> → None maps to SQL NULL
        let zoom: Option<f64> = zoom_scale.map(|z| z as f64);
        rt().block_on(self.conn.execute(
            "INSERT INTO model_history (file_path, name, model_version, zoom_scale) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(file_path) DO UPDATE SET \
                name=?2, model_version=?3, zoom_scale=?4, last_opened=datetime('now')",
            libsql::params![file_path, name, model_version, zoom],
        ))?;
        Ok(())
    }

    /// Return every model history record ordered by `last_opened`
    /// descending (most recent first).
    pub fn model_history(&self) -> Result<Vec<ModelRecord>> {
        let mut rows = rt().block_on(self.conn.query(
            "SELECT file_path, name, model_version, zoom_scale, last_opened \
             FROM model_history ORDER BY last_opened DESC",
            (),
        ))?;

        let mut records = Vec::new();
        while let Some(row) = rt().block_on(rows.next())? {
            records.push(ModelRecord {
                file_path: row.get::<String>(0)?,
                name: row.get::<String>(1)?,
                model_version: row.get::<String>(2)?,
                zoom_scale: row.get::<Option<f64>>(3)?.map(|z| z as f32),
                last_opened: row.get::<String>(4)?,
            });
        }
        Ok(records)
    }

    /// Retrieve a single model record by its file path.
    pub fn get_model(&self, file_path: &str) -> Result<Option<ModelRecord>> {
        let mut rows = rt().block_on(self.conn.query(
            "SELECT file_path, name, model_version, zoom_scale, last_opened \
             FROM model_history WHERE file_path = ?1",
            libsql::params![file_path],
        ))?;

        match rt().block_on(rows.next())? {
            Some(row) => Ok(Some(ModelRecord {
                file_path: row.get::<String>(0)?,
                name: row.get::<String>(1)?,
                model_version: row.get::<String>(2)?,
                zoom_scale: row.get::<Option<f64>>(3)?.map(|z| z as f32),
                last_opened: row.get::<String>(4)?,
            })),
            None => Ok(None),
        }
    }

    /// Update the zoom scale for a model. Pass `None` to clear the value.
    pub fn set_zoom(&self, file_path: &str, zoom_scale: Option<f32>) -> Result<()> {
        let zoom: Option<f64> = zoom_scale.map(|z| z as f64);
        rt().block_on(self.conn.execute(
            "UPDATE model_history SET zoom_scale = ?1 WHERE file_path = ?2",
            libsql::params![zoom, file_path],
        ))?;
        Ok(())
    }

    /// Remove a model from the history (e.g. when its directory no longer
    /// exists on disk).
    pub fn remove_model(&self, file_path: &str) -> Result<()> {
        rt().block_on(self.conn.execute(
            "DELETE FROM model_history WHERE file_path = ?1",
            libsql::params![file_path],
        ))?;
        Ok(())
    }

    /// Rename a model in the history (user-friendly display name).
    pub fn rename_model(&self, file_path: &str, new_name: &str) -> Result<()> {
        rt().block_on(self.conn.execute(
            "UPDATE model_history SET name = ?1 WHERE file_path = ?2",
            libsql::params![new_name, file_path],
        ))?;
        Ok(())
    }

    /// Given the current model's `file_path`, return the previous and next
    /// model paths in stable `file_path ASC` order (wrapping around).
    /// Returns `(prev, next)` or `None` if only one record exists.
    pub fn prev_next_paths(&self, current: &str) -> Result<Option<(String, String)>> {
        let mut rows = rt().block_on(self.conn.query(
            "SELECT file_path FROM model_history ORDER BY file_path ASC",
            (),
        ))?;
        let mut paths = Vec::new();
        while let Some(row) = rt().block_on(rows.next())? {
            paths.push(row.get::<String>(0)?);
        }
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

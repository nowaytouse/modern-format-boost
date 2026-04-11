//! 🗄️ Media Index System - `SQLite` Backend (Dev-only)
//!
//! Accelerates development by flattening physical conversion costs into a structured DB.
//! Relocated to crates/dev to separate development auditing from production code.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use shared_utils::media_index_types::MediaIndexRow;

pub struct MediaIndex {
    pub conn: Connection,
}

impl MediaIndex {
    /// Opens or creates the media index database at the specified path.
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened or schema initialization fails.
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open MediaIndex at {}", db_path.display()))?;

        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS media_entries (
                blake3 TEXT PRIMARY KEY,
                rel_path TEXT NOT NULL,
                media_type TEXT NOT NULL,
                width INTEGER NOT NULL,
                height INTEGER NOT NULL,
                format TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                has_hdr BOOLEAN NOT NULL,
                has_alpha BOOLEAN NOT NULL,
                duration REAL NOT NULL,
                raw_features_json TEXT NOT NULL,
                decided_format TEXT,
                decided_params_json TEXT,
                decision_reason TEXT,
                flagged_issue TEXT,
                last_extracted_at INTEGER NOT NULL
            )",
            [],
        )?;

        // 🛡️ Table for version-controlled snapshots of decisions
        conn.execute(
            "CREATE TABLE IF NOT EXISTS decision_snapshots (
                blake3 TEXT NOT NULL,
                version_tag TEXT NOT NULL,
                decided_format TEXT NOT NULL,
                decided_params_json TEXT NOT NULL,
                decision_reason TEXT NOT NULL,
                snapshot_at INTEGER NOT NULL,
                PRIMARY KEY (blake3, version_tag)
            )",
            [],
        )?;

        // 📡 Table for recording real-world production decisions
        conn.execute(
            "CREATE TABLE IF NOT EXISTS live_audit (
                blake3 TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                actual_format TEXT NOT NULL,
                actual_params_json TEXT NOT NULL,
                vmaf_score REAL,
                audit_at INTEGER NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_media_type ON media_entries(media_type)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_format ON media_entries(format)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_snapshot_tag ON decision_snapshots(version_tag)",
            [],
        )?;

        Ok(())
    }

    /// Upserts a raw extraction record (only overwrites immutable features).
    ///
    /// # Errors
    /// Returns an error if the database update fails.
    pub fn upsert_extraction(&self, row: &MediaIndexRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO media_entries (
                blake3, rel_path, media_type, width, height, format, 
                file_size, has_hdr, has_alpha, duration, raw_features_json, last_extracted_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(blake3) DO UPDATE SET
                rel_path = EXCLUDED.rel_path,
                width = EXCLUDED.width,
                height = EXCLUDED.height,
                format = EXCLUDED.format,
                file_size = EXCLUDED.file_size,
                has_hdr = EXCLUDED.has_hdr,
                has_alpha = EXCLUDED.has_alpha,
                duration = EXCLUDED.duration,
                raw_features_json = EXCLUDED.raw_features_json,
                last_extracted_at = EXCLUDED.last_extracted_at",
            params![
                row.blake3,
                row.rel_path,
                row.media_type,
                row.width,
                row.height,
                row.format,
                row.file_size,
                row.has_hdr,
                row.has_alpha,
                row.duration,
                row.raw_features_json,
                row.last_extracted_at
            ],
        )?;
        Ok(())
    }

    /// Updates the decision columns for a specific record.
    ///
    /// # Errors
    /// Returns an error if the database update fails.
    pub fn update_decision(
        &self,
        blake3: &str,
        format: &str,
        params_json: &str,
        reason: &str,
        issue: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE media_entries SET 
                decided_format = ?2, 
                decided_params_json = ?3, 
                decision_reason = ?4,
                flagged_issue = ?5
             WHERE blake3 = ?1",
            params![blake3, format, params_json, reason, issue],
        )?;
        Ok(())
    }

    /// Retrieves a single record by content hash.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub fn get_record(&self, blake3: &str) -> Result<Option<MediaIndexRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM media_entries WHERE blake3 = ?1")?;
        let mut rows = stmt.query_map(params![blake3], |row| {
            Ok(MediaIndexRow {
                blake3: row.get(0)?,
                rel_path: row.get(1)?,
                media_type: row.get(2)?,
                width: row.get(3)?,
                height: row.get(4)?,
                format: row.get(5)?,
                file_size: row.get(6)?,
                has_hdr: row.get(7)?,
                has_alpha: row.get(8)?,
                duration: row.get(9)?,
                raw_features_json: row.get(10)?,
                decided_format: row.get(11)?,
                decided_params_json: row.get(12)?,
                decision_reason: row.get(13)?,
                flagged_issue: row.get(14)?,
                last_extracted_at: row.get(15)?,
            })
        })?;

        if let Some(res) = rows.next() {
            return Ok(Some(res?));
        }
        Ok(None)
    }

    /// Returns a count of records in the index.
    ///
    /// # Errors
    /// Returns an error if the database query fails.
    pub fn count_records(&self) -> Result<usize> {
        let count: usize =
            self.conn
                .query_row("SELECT COUNT(*) FROM media_entries", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Returns a raw statement for the inner connection.
    ///
    /// # Errors
    /// Returns an error if the SQL is invalid.
    pub fn conn_prepare(
        &self,
        sql: &str,
    ) -> std::result::Result<rusqlite::Statement<'_>, rusqlite::Error> {
        self.conn.prepare(sql)
    }

    /// 🛡️ Snapshots all CURRENT decisions from `media_entries` into `decision_snapshots` under a tag.
    ///
    /// # Errors
    /// Returns an error if the database update fails.
    pub fn save_snapshot(&self, version_tag: &str) -> Result<()> {
        let ts = now_unix();
        self.conn.execute(
            "INSERT OR REPLACE INTO decision_snapshots (
                blake3, version_tag, decided_format, decided_params_json, decision_reason, snapshot_at
            )
            SELECT blake3, ?1, decided_format, decided_params_json, decision_reason, ?2
            FROM media_entries
            WHERE decided_format IS NOT NULL",
            params![version_tag, ts],
        )?;
        Ok(())
    }

    /// 📡 Logs a real-world production decision for audit/drift analysis.
    ///
    /// # Errors
    /// Returns an error if the database update fails.
    pub fn log_live_details(
        &self,
        blake3: &str,
        session_id: &str,
        actual_format: &str,
        params_json: &str,
        vmaf: Option<f64>,
    ) -> Result<()> {
        let ts = now_unix();
        self.conn.execute(
            "INSERT OR REPLACE INTO live_audit (
                blake3, session_id, actual_format, actual_params_json, vmaf_score, audit_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![blake3, session_id, actual_format, params_json, vmaf, ts],
        )?;
        Ok(())
    }

    /// ⚡ Zero-overhead check: Returns true if a `MediaIndex` exists at the path.
    pub fn exists_at(db_path: &Path) -> bool {
        db_path.exists() && db_path.is_file()
    }
}

/// Helper to get current unix time.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

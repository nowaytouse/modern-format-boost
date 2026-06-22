//! 🗄 Media Index System - `SQLite` Backend (Dev-only)
//!
//! Accelerates development by flattening physical conversion costs into a structured DB.
//! Relocated to crates/dev to separate development auditing from production code.

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use foundation::media_index_types::MediaIndexRow;

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

        foundation::log_success!(
            "MediaIndex",
            &format!(
                "SQLite development index successfully opened at {path}; caching layer is active.",
                path = db_path.display()
            )
        );

        Self::init_schema(&conn)?;
        Ok(Self { conn })
    }

    /// Opens the unified local store (`mfb_store.sqlite`).
    ///
    /// # Errors
    /// Returns an error if the store cannot be opened.
    pub fn open_default() -> Result<Self> {
        let path = foundation::mfb_sqlite_store::default_store_path()?;
        Self::open(&path)
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        foundation::log_info!(
            "MediaIndex",
            "Initializing SQLite development schema; verifying 'media_entries' table and indices..."
        );
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

        // Table for version-controlled snapshots of decisions
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

        // Table for recording real-world production decisions
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

        foundation::log_success!(
            "MediaIndex",
            "Development schema verification complete; media_entries and audit tables are ready for caching."
        );

        Ok(())
    }
    /// Upserts a raw extraction record (only overwrites immutable features).
    ///
    /// # Errors
    /// Returns an error if the database update fails.
    pub fn upsert_extraction(&self, row: &MediaIndexRow) -> Result<()> {
        foundation::log_detail!(&format!(
            "Upserting extraction record for {path} (hash: {blake3}) into development index.",
            path = row.rel_path,
            blake3 = row.blake3
        ));
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
                row.blake3.clone(),
                row.rel_path.clone(),
                row.media_type.clone(),
                i64::from(row.width),
                i64::from(row.height),
                row.format.clone(),
                foundation::numeric_cast::u64_to_i64_sat(row.file_size),
                row.has_hdr,
                row.has_alpha,
                row.duration,
                row.raw_features_json.clone(),
                now_unix(),
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
            let width_raw: i64 = row.get(3)?;
            let height_raw: i64 = row.get(4)?;
            let file_size_raw: i64 = row.get(6)?;

            Ok(MediaIndexRow {
                blake3: row.get(0)?,
                rel_path: row.get(1)?,
                media_type: row.get(2)?,
                width: foundation::numeric_cast::i64_to_u32_sat(width_raw),
                height: foundation::numeric_cast::i64_to_u32_sat(height_raw),
                format: row.get(5)?,
                file_size: foundation::numeric_cast::i64_to_u64_sat(file_size_raw),
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
        let count_raw: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM media_entries", [], |row| row.get(0))?;
        Ok(foundation::numeric_cast::i64_to_usize_sat(count_raw))
    }

    /// Returns a raw statement for the inner connection.
    ///
    /// # Errors
    /// Returns an error if the SQL is invalid.
    pub fn conn_prepare(
        &self,
        sql: &str,
    ) -> core::result::Result<rusqlite::Statement<'_>, rusqlite::Error> {
        self.conn.prepare(sql)
    }

    /// Snapshots all CURRENT decisions from `media_entries` into `decision_snapshots` under a tag.
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

    /// Logs a real-world production decision for audit/drift analysis.
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

    ///  Zero-overhead check: Returns true if a `MediaIndex` exists at the path.
    #[must_use]
    pub fn exists_at(db_path: &Path) -> bool {
        db_path.exists() && db_path.is_file()
    }
}

/// Helper to get current unix time.
#[must_use]
/// # Panics
///
/// Panics if the current system time is out of range for a 64-bit signed integer.
pub fn now_unix() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system time before UNIX_EPOCH: {err}"))
            .as_secs(),
    )
    .expect("Failed to parse integer or missing required value")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row(has_hdr: bool, raw_features_json: &str) -> MediaIndexRow {
        MediaIndexRow {
            blake3: "hash123".to_string(),
            rel_path: "library/sample.mov".to_string(),
            media_type: "video".to_string(),
            width: 1920,
            height: 1080,
            format: "mov".to_string(),
            file_size: 1_024,
            has_hdr,
            has_alpha: false,
            duration: 120.0,
            raw_features_json: raw_features_json.to_string(),
            decided_format: None,
            decided_params_json: None,
            decision_reason: None,
            flagged_issue: None,
            last_extracted_at: now_unix(),
        }
    }

    #[test]
    fn upsert_extraction_refreshes_existing_hdr_state() {
        let tempdir = tempfile::tempdir().expect("create temp dir");
        let db_path = tempdir.path().join("media_index.sqlite");
        let db = MediaIndex::open(&db_path).expect("open media index");

        db.upsert_extraction(&sample_row(false, r#"{"has_hdr":false}"#))
            .expect("insert initial row");

        db.upsert_extraction(&sample_row(true, r#"{"has_hdr":true}"#))
            .expect("refresh existing row");

        let row = db
            .get_record("hash123")
            .expect("load refreshed row")
            .expect("row must exist");
        assert!(row.has_hdr);
        assert_eq!(row.raw_features_json, r#"{"has_hdr":true}"#);
        assert_eq!(db.count_records().expect("count rows"), 1);
    }
}

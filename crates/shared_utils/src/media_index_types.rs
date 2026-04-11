use serde::{Deserialize, Serialize};

/// 🎥 Central record representing a single media asset in the library.
/// Shared between production logic (recommenders) and dev logic (database).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaIndexRow {
    /// BLAKE3 hash of the file content (Primary Key).
    pub blake3: String,
    /// Path relative to the library root.
    pub rel_path: String,
    /// 'image' or 'video'.
    pub media_type: String,

    // Core physical features (Immutable once extracted)
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub file_size: u64,
    pub has_hdr: bool,
    pub has_alpha: bool,
    pub duration: f64,

    /// Full JSON dump of original detection results (`DetectionResult` or `VideoDetectionResult`).
    pub raw_features_json: String,

    // Decision outcomes (Mutable during development/debugging)
    pub decided_format: Option<String>,
    pub decided_params_json: Option<String>,
    pub decision_reason: Option<String>,
    pub flagged_issue: Option<String>,

    /// Unix timestamp of extraction.
    pub last_extracted_at: i64,
}

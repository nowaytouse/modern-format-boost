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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_index_row_serde() {
        let row = MediaIndexRow {
            blake3: "hash123".to_string(),
            rel_path: "path/to/file.jpg".to_string(),
            media_type: "image".to_string(),
            width: 1920,
            height: 1080,
            format: "JPEG".to_string(),
            file_size: 1024,
            has_hdr: false,
            has_alpha: false,
            duration: 0.0,
            raw_features_json: "{}".to_string(),
            decided_format: Some("JXL".to_string()),
            decided_params_json: None,
            decision_reason: None,
            flagged_issue: None,
            last_extracted_at: 1_600_000_000,
        };

        let json = serde_json::to_string(&row).unwrap_or_else(|e| {
            unreachable!(
                "CRITICAL: MediaIndexRow serialization failed in test (error: {:?})",
                e
            )
        });
        assert!(json.contains("hash123"));

        let deserialized: MediaIndexRow = serde_json::from_str(&json).unwrap_or_else(|e| {
            unreachable!(
                "CRITICAL: MediaIndexRow deserialization failed in test (error: {:?})",
                e
            )
        });
        assert_eq!(deserialized.blake3, "hash123");
        assert_eq!(deserialized.width, 1920);
        assert_eq!(deserialized.decided_format, Some("JXL".to_string()));
        assert_eq!(deserialized.decided_params_json, None);
    }
}

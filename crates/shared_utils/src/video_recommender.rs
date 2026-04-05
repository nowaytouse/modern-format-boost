use crate::media_index_types::MediaIndexRow;
use crate::video_detection::{DetectedCodec, VideoDetectionResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoRecommendation {
    pub current_codec: String,
    pub recommended_codec: String,
    pub reason: String,
    pub is_archival_upgrade: bool,
    pub command_hint: String,
}

/// 🚀 New Entry Point: Subscribes to MediaIndexRow (Database-driven decision)
#[must_use]
pub fn get_video_recommendation_from_row(row: &MediaIndexRow) -> Result<VideoRecommendation, serde_json::Error> {
    let features: VideoDetectionResult = serde_json::from_str(&row.raw_features_json)?;
    
    Ok(generate_video_recommendation(&features))
}

fn generate_video_recommendation(features: &VideoDetectionResult) -> VideoRecommendation {
    let mut recommended_codec = features.codec.as_str().to_string();
    let mut reason = "Current codec is optimal or sufficient".to_string();
    let mut is_archival_upgrade = false;
    let mut command_hint = String::new();

    // Decision Logic: If it's a high-fidelity archival candidate but not yet in modern modern formats
    let is_old_lossless = matches!(features.codec, DetectedCodec::ProRes | DetectedCodec::DNxHD | DetectedCodec::MJPEG);
    let is_high_bitrate_h264 = features.codec == DetectedCodec::H264 && features.bitrate > 50_000_000;

    if is_old_lossless || is_high_bitrate_h264 {
        recommended_codec = "AV1 (SVT-AV1)".to_string();
        is_archival_upgrade = true;
        reason = if is_old_lossless {
            "Professional archival format detected; recommend AV1 for space efficiency with zero visual loss".to_string()
        } else {
            "High-bitrate H.264 detected; recommend AV1 for 50%+ size reduction".to_string()
        };
        command_hint = format!("ffmpeg -i '{}' -c:v libsvtav1 -preset 6 -crf 20 output.mp4", features.file_path);
    }

    VideoRecommendation {
        current_codec: features.codec.as_str().to_string(),
        recommended_codec,
        reason,
        is_archival_upgrade,
        command_hint,
    }
}

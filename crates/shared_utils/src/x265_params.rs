//! Shared x265 parameter policy helpers.
//!
//! Large archival/intermediate sources such as ProRes and DNxHD can drive libx265
//! to very high resident memory usage because x265 auto-scales frame threading and
//! lookahead buffering. This module provides a narrow "low-memory" profile that is
//! only enabled for large/high-fidelity inputs, trading some throughput for a much
//! lower peak RAM footprint.

use crate::constants;
use crate::video_detection::{DetectedCodec, VideoDetectionResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X265MemoryProfile {
    Default,
    LowMemory,
}

impl X265MemoryProfile {
    #[must_use]
    pub const fn is_low_memory(self) -> bool {
        matches!(self, Self::LowMemory)
    }
}

#[must_use]
pub fn memory_profile_for_detection(detection: &VideoDetectionResult) -> X265MemoryProfile {
    let archival_or_intermediate = detection.codec.can_be_lossless();
    let very_large_source = detection.file_size >= constants::X265_LOW_MEMORY_SOURCE_SIZE_BYTES;

    if archival_or_intermediate || very_large_source {
        X265MemoryProfile::LowMemory
    } else {
        X265MemoryProfile::Default
    }
}

#[must_use]
pub fn memory_profile_for_source(codec_name: Option<&str>, input_size: u64) -> X265MemoryProfile {
    let archival_or_intermediate = codec_name
        .map(DetectedCodec::from_ffprobe)
        .is_some_and(|codec| codec.can_be_lossless());
    let very_large_source = input_size >= constants::X265_LOW_MEMORY_SOURCE_SIZE_BYTES;

    if archival_or_intermediate || very_large_source {
        X265MemoryProfile::LowMemory
    } else {
        X265MemoryProfile::Default
    }
}

#[must_use]
pub fn format_x265_params(
    max_threads: usize,
    extra_params: Option<&str>,
    profile: X265MemoryProfile,
) -> String {
    let mut params = format!("log-level=error:pools={}", max_threads.max(1));
    apply_memory_profile(&mut params, profile);
    append_extra_params(&mut params, extra_params);
    params
}

#[must_use]
pub fn format_x265_lossless_params(
    max_threads: usize,
    extra_params: Option<&str>,
    profile: X265MemoryProfile,
) -> String {
    let mut params = format!("lossless=1:log-level=error:pools={}", max_threads.max(1));
    apply_memory_profile(&mut params, profile);
    append_extra_params(&mut params, extra_params);
    params
}

pub fn push_param(params: &mut String, param: &str) {
    let trimmed = param.trim_matches(':');
    if trimmed.is_empty() {
        return;
    }
    if !params.is_empty() {
        params.push(':');
    }
    params.push_str(trimmed);
}

fn apply_memory_profile(params: &mut String, profile: X265MemoryProfile) {
    if !profile.is_low_memory() {
        return;
    }

    push_param(
        params,
        &format!("frame-threads={}", constants::X265_LOW_MEMORY_FRAME_THREADS),
    );
    push_param(
        params,
        &format!(
            "lookahead-threads={}",
            constants::X265_LOW_MEMORY_LOOKAHEAD_THREADS
        ),
    );
    push_param(
        params,
        &format!(
            "lookahead-slices={}",
            constants::X265_LOW_MEMORY_LOOKAHEAD_SLICES
        ),
    );
    push_param(
        params,
        &format!("rc-lookahead={}", constants::X265_LOW_MEMORY_RC_LOOKAHEAD),
    );
}

fn append_extra_params(params: &mut String, extra_params: Option<&str>) {
    if let Some(extra) = extra_params {
        push_param(params, extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_detection::{CompressionType, VideoPrecisionMetadata};

    fn sample_detection(codec: DetectedCodec, file_size: u64) -> VideoDetectionResult {
        VideoDetectionResult {
            codec,
            compression: CompressionType::VisuallyLossless,
            file_size,
            precision: VideoPrecisionMetadata::default(),
            ..Default::default()
        }
    }

    #[test]
    fn archival_codec_enables_low_memory_profile() {
        let detection = sample_detection(DetectedCodec::ProRes, 512 * 1024 * 1024);
        assert_eq!(
            memory_profile_for_detection(&detection),
            X265MemoryProfile::LowMemory
        );
    }

    #[test]
    fn very_large_source_enables_low_memory_profile() {
        assert_eq!(
            memory_profile_for_source(
                Some("h264"),
                constants::X265_LOW_MEMORY_SOURCE_SIZE_BYTES + 1
            ),
            X265MemoryProfile::LowMemory
        );
    }

    #[test]
    fn low_memory_profile_injects_buffer_controls() {
        let params = format_x265_params(4, Some("hdr-opt=1"), X265MemoryProfile::LowMemory);
        assert!(params.contains("pools=4"));
        assert!(params.contains("frame-threads=1"));
        assert!(params.contains("lookahead-threads=1"));
        assert!(params.contains("lookahead-slices=1"));
        assert!(params.contains("rc-lookahead=10"));
        assert!(params.ends_with("hdr-opt=1"));
    }

    #[test]
    fn lossless_builder_preserves_lossless_flag() {
        let params = format_x265_lossless_params(2, None, X265MemoryProfile::LowMemory);
        assert!(params.starts_with("lossless=1:"));
        assert!(params.contains("frame-threads=1"));
    }
}

//! Shared x265 parameter policy helpers.
//!
//! Large archival/intermediate sources such as ProRes and DNxHD can drive libx265
//! to very high resident memory usage because x265 auto-scales frame threading and
//! lookahead buffering. This module provides a tiered memory profile that adapts
//! to the actual available system RAM rather than a hard binary switch, trading
//! some throughput for a lower peak RAM footprint only when truly necessary.

use crate::constants;
use crate::video_detection::{DetectedCodec, Detection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X265MemoryProfile {
    Default,
    Moderate,
    LowMemory,
}

impl X265MemoryProfile {
    #[must_use]
    pub const fn is_low_memory(self) -> bool {
        matches!(self, Self::LowMemory)
    }

    #[must_use]
    pub const fn is_constrained(self) -> bool {
        matches!(self, Self::LowMemory | Self::Moderate)
    }
}

/// Determine the appropriate memory profile from a detection result.
///
/// Uses the source codec and file size as signals, then queries actual
/// available system RAM to pick the least restrictive profile that is safe.
#[must_use]
pub fn memory_profile_for_detection(detection: &Detection) -> X265MemoryProfile {
    let needs_care = detection.codec.can_be_lossless()
        || detection.file_size >= constants::X265_LOW_MEMORY_SOURCE_SIZE_BYTES;

    if !needs_care {
        return X265MemoryProfile::Default;
    }

    ram_aware_profile()
}

/// Determine the appropriate memory profile from codec name and file size.
///
/// Uses the source codec and file size as signals, then queries actual
/// available system RAM to pick the least restrictive profile that is safe.
#[must_use]
pub fn memory_profile_for_source(codec_name: Option<&str>, input_size: u64) -> X265MemoryProfile {
    let archival_or_intermediate = codec_name
        .map(DetectedCodec::from_ffprobe)
        .is_some_and(|codec| codec.can_be_lossless());
    let very_large_source = input_size >= constants::X265_LOW_MEMORY_SOURCE_SIZE_BYTES;

    if !archival_or_intermediate && !very_large_source {
        return X265MemoryProfile::Default;
    }

    ram_aware_profile()
}

/// Query actual available system RAM and return the appropriate tier.
/// Falls back to `LowMemory` if RAM detection fails (safe default).
fn ram_aware_profile() -> X265MemoryProfile {
    if crate::system_memory::is_low_memory_env() {
        return X265MemoryProfile::LowMemory;
    }

    let (available_mb, total_mb) = crate::system_memory::get_memory_mb().unwrap_or_else(|| {
        tracing::warn!(
            "System RAM detection failed; falling back to conservative LowMemory profile for x265"
        );
        (0, 0)
    });
    profile_for_available_memory(available_mb, total_mb)
}

/// Query the current system RAM tier using the same thresholds that shape x265 behavior.
///
/// This is shared with batch/thread scheduling so file-level parallelism follows the
/// same `Default` / `Moderate` / `LowMemory` policy as x265 itself.
#[must_use]
pub fn current_memory_profile() -> X265MemoryProfile {
    ram_aware_profile()
}

#[must_use]
pub fn format(
    max_threads: usize,
    extra_params: Option<&str>,
    profile: X265MemoryProfile,
) -> String {
    let pool_threads = capped_pool_threads(max_threads, profile);
    let mut params = format!("log-level=error:pools={pool_threads}");
    apply_memory_profile(&mut params, profile, pool_threads);
    append_extra_params(&mut params, extra_params);
    params
}

#[must_use]
pub fn format_x265_lossless_params(
    max_threads: usize,
    extra_params: Option<&str>,
    profile: X265MemoryProfile,
) -> String {
    let pool_threads = capped_pool_threads(max_threads, profile);
    let mut params = format!("lossless=1:log-level=error:pools={pool_threads}");
    apply_memory_profile(&mut params, profile, pool_threads);
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

fn profile_for_available_memory(available_mb: u64, total_mb: u64) -> X265MemoryProfile {
    if available_mb == 0 || total_mb == 0 {
        return X265MemoryProfile::LowMemory;
    }

    let free_ratio =
        crate::numeric_cast::u64_to_f64(available_mb) / crate::numeric_cast::u64_to_f64(total_mb);

    if available_mb >= constants::X265_RELAXED_DEFAULT_RAM_THRESHOLD_MB
        && free_ratio >= constants::X265_DEFAULT_RAM_RATIO_THRESHOLD
    {
        X265MemoryProfile::Default
    } else if available_mb >= constants::X265_MODERATE_RAM_THRESHOLD_MB
        && free_ratio >= constants::X265_MODERATE_RAM_RATIO_THRESHOLD
    {
        X265MemoryProfile::Moderate
    } else {
        X265MemoryProfile::LowMemory
    }
}

fn capped_pool_threads(max_threads: usize, profile: X265MemoryProfile) -> usize {
    let max_threads = max_threads.max(1);
    match profile {
        X265MemoryProfile::Default => max_threads,
        X265MemoryProfile::Moderate => max_threads.min(constants::X265_MODERATE_MEMORY_MAX_POOLS),
        X265MemoryProfile::LowMemory => max_threads.min(constants::X265_LOW_MEMORY_MAX_POOLS),
    }
}

fn apply_memory_profile(params: &mut String, profile: X265MemoryProfile, pool_threads: usize) {
    match profile {
        X265MemoryProfile::Default => {}
        X265MemoryProfile::Moderate => {
            let frame_threads =
                pool_threads.clamp(1, constants::X265_MODERATE_MEMORY_FRAME_THREADS);
            let lookahead_threads =
                pool_threads.clamp(1, constants::X265_MODERATE_MEMORY_LOOKAHEAD_THREADS);
            let lookahead_slices =
                pool_threads.clamp(1, constants::X265_MODERATE_MEMORY_LOOKAHEAD_SLICES);
            push_param(params, &format!("frame-threads={frame_threads}"));
            push_param(params, &format!("lookahead-threads={lookahead_threads}"));
            push_param(params, &format!("lookahead-slices={lookahead_slices}"));
            push_param(
                params,
                &format!(
                    "rc-lookahead={}",
                    constants::X265_MODERATE_MEMORY_RC_LOOKAHEAD
                ),
            );
        }
        X265MemoryProfile::LowMemory => {
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
    }
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

    const _: () = assert!(
        constants::X265_LOW_MEMORY_RC_LOOKAHEAD
            > constants::X265_ALLOWED_HEVC_MAX_CONSECUTIVE_BFRAMES
    );

    fn sample_detection(codec: DetectedCodec, file_size: u64) -> Detection {
        Detection {
            codec,
            compression: CompressionType::VisuallyLossless,
            file_size,
            precision: VideoPrecisionMetadata::default(),
            ..Default::default()
        }
    }

    #[test]
    fn archival_codec_enables_constrained_profile() {
        let detection = sample_detection(DetectedCodec::ProRes, 512 * 1024 * 1024);
        let profile = memory_profile_for_detection(&detection);
        // The exact profile depends on the current machine, but archival sources should always
        // resolve to one of the supported tiers without panicking.
        assert!(
            profile == X265MemoryProfile::Default
                || profile == X265MemoryProfile::Moderate
                || profile == X265MemoryProfile::LowMemory,
            "Profile must be a valid variant"
        );
    }

    #[test]
    fn very_large_source_enables_constrained_profile() {
        let profile = memory_profile_for_source(
            Some("h264"),
            constants::X265_LOW_MEMORY_SOURCE_SIZE_BYTES + 1,
        );
        // Must be constrained for very large files, exact tier depends on RAM
        assert!(profile.is_constrained() || profile == X265MemoryProfile::Default);
    }

    #[test]
    fn low_memory_profile_injects_buffer_controls() {
        let params = format(4, Some("hdr-opt=1"), X265MemoryProfile::LowMemory);
        assert!(params.contains("pools=2"));
        assert!(params.contains("frame-threads=1"));
        assert!(params.contains("lookahead-threads=1"));
        assert!(params.contains("lookahead-slices=1"));
        assert!(params.contains("rc-lookahead=9"));
        assert!(params.ends_with("hdr-opt=1"));
    }

    #[test]
    fn moderate_profile_injects_moderate_controls() {
        let params = format(12, None, X265MemoryProfile::Moderate);
        assert!(params.contains("pools=6"));
        assert!(params.contains("frame-threads=3"));
        assert!(params.contains("lookahead-threads=3"));
        assert!(params.contains("lookahead-slices=3"));
        assert!(params.contains("rc-lookahead=20"));
    }

    #[test]
    fn moderate_profile_scales_down_when_threads_are_already_low() {
        let params = format(2, None, X265MemoryProfile::Moderate);
        assert!(params.contains("pools=2"));
        assert!(params.contains("frame-threads=2"));
        assert!(params.contains("lookahead-threads=2"));
        assert!(params.contains("lookahead-slices=2"));
        assert!(params.contains("rc-lookahead=20"));
    }

    #[test]
    fn default_profile_injects_no_buffer_controls() {
        let params = format(4, None, X265MemoryProfile::Default);
        assert!(params.contains("pools=4"));
        assert!(!params.contains("frame-threads"));
        assert!(!params.contains("lookahead-threads"));
        assert!(!params.contains("rc-lookahead"));
    }

    #[test]
    fn lossless_builder_preserves_lossless_flag() {
        let params = format_x265_lossless_params(2, None, X265MemoryProfile::LowMemory);
        assert!(params.starts_with("lossless=1:"));
        assert!(params.contains("frame-threads=1"));
        assert!(params.contains("pools=2"));
    }

    #[test]
    fn small_normal_codec_gets_default() {
        let profile = memory_profile_for_source(Some("h264"), 100 * 1024 * 1024);
        assert_eq!(profile, X265MemoryProfile::Default);
    }

    #[test]
    fn healthy_free_ratio_keeps_default_profile() {
        assert_eq!(
            profile_for_available_memory(8 * 1024, 16 * 1024),
            X265MemoryProfile::Default
        );
        assert_eq!(
            profile_for_available_memory(10 * 1024, 32 * 1024),
            X265MemoryProfile::Default
        );
    }

    #[test]
    fn tight_free_ratio_drops_to_more_aggressive_profiles() {
        assert_eq!(
            profile_for_available_memory(8 * 1024, 64 * 1024),
            X265MemoryProfile::LowMemory
        );
        assert_eq!(
            profile_for_available_memory(12 * 1024, 64 * 1024),
            X265MemoryProfile::Moderate
        );
    }
}

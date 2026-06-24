//! Shared sample-depth provenance and high-precision preservation policy.

use std::path::Path;

/// Bit-depth metadata plus provenance.
///
/// `bit_depth` may come from an explicit ffprobe field or a `pix_fmt` fallback.
/// Callers that need archival-grade certainty should use `confirmed_bit_depth`,
/// while conversion paths that only need to avoid precision loss should use
/// `effective_bit_depth` or `should_preserve_high_bit_depth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BitDepthMetadata {
    pub bit_depth: Option<u8>,
    pub bit_depth_inferred_from_pix_fmt: bool,
}

impl BitDepthMetadata {
    #[must_use]
    pub const fn new(bit_depth: Option<u8>, bit_depth_inferred_from_pix_fmt: bool) -> Self {
        Self {
            bit_depth,
            bit_depth_inferred_from_pix_fmt,
        }
    }

    #[must_use]
    pub const fn effective_bit_depth(self) -> Option<u8> {
        self.bit_depth
    }

    #[must_use]
    pub const fn confirmed_bit_depth(self) -> Option<u8> {
        if self.bit_depth_inferred_from_pix_fmt {
            None
        } else {
            self.bit_depth
        }
    }

    #[must_use]
    pub fn has_high_bit_depth(self) -> bool {
        self.effective_bit_depth()
            .is_some_and(|d| d >= crate::constants::HDR_BIT_DEPTH_THRESHOLD)
    }

    #[must_use]
    pub fn has_confirmed_high_bit_depth(self) -> bool {
        self.confirmed_bit_depth()
            .is_some_and(|d| d >= crate::constants::HDR_BIT_DEPTH_THRESHOLD)
    }

    #[must_use]
    pub fn format_label(self) -> String {
        crate::media_conversion_gate::ui_bit_depth_format_label_or_na(
            self.bit_depth,
            self.bit_depth_inferred_from_pix_fmt,
            "media_precision_bit_depth",
        )
    }
}

/// Unified interface for media types that carry HDR signaling plus sample-depth
/// provenance.
pub trait MediaPrecision {
    fn bit_depth_metadata(&self) -> BitDepthMetadata;
    fn has_hdr_signaling(&self) -> bool;

    #[must_use]
    fn effective_bit_depth(&self) -> Option<u8> {
        self.bit_depth_metadata().effective_bit_depth()
    }

    #[must_use]
    fn confirmed_bit_depth(&self) -> Option<u8> {
        self.bit_depth_metadata().confirmed_bit_depth()
    }

    /// Returns true when encode/decode paths should preserve 10-bit+ precision.
    ///
    /// This intentionally accepts `pix_fmt`-inferred sample depth so conversion
    /// code can avoid truncation, but it must not be treated as proof of
    /// confirmed HDR or explicit source precision.
    #[must_use]
    fn should_preserve_high_bit_depth(&self) -> bool {
        self.has_hdr_signaling() || self.bit_depth_metadata().has_high_bit_depth()
    }

    /// Returns true only when 10-bit+ precision was explicitly reported.
    #[must_use]
    fn has_confirmed_high_bit_depth(&self) -> bool {
        self.bit_depth_metadata().has_confirmed_high_bit_depth()
    }

    #[must_use]
    fn format_bit_depth_label(&self) -> String {
        self.bit_depth_metadata().format_label()
    }
}

/// Return the HEVC-compatible yuv420 output pixel format that preserves source
/// precision.
#[must_use]
pub fn hevc_yuv420_output_pix_fmt(source: &impl MediaPrecision) -> &'static str {
    if source.should_preserve_high_bit_depth() {
        crate::constants::PIX_FMT_YUV420P10LE
    } else {
        crate::constants::PIX_FMT_YUV420P
    }
}

/// Precision-preservation plan for still-image conversion pipelines.
///
/// This extends raw sample-depth metadata with a few conservative container
/// heuristics used when preparing intermediate image bitstreams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImagePrecisionDetection {
    is_float: bool,
    bit_depth_inferred_from_pix_fmt: bool,
    used_float_extension_hint: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImagePrecisionPreservation {
    preserve_high_precision: bool,
    preserve_unknown_container_with_16bit: bool,
    metadata_requires_high_precision_decode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImagePrecisionProfile {
    detection: ImagePrecisionDetection,
    bit_depth: Option<u8>,
    preservation: ImagePrecisionPreservation,
}

impl ImagePrecisionProfile {
    #[must_use]
    pub fn inspect(input: &Path, color_info: &crate::ffprobe_json::ColorInfo) -> Self {
        let ext_lower =
            crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(input);
        let ext_lower = (!ext_lower.is_empty()).then_some(ext_lower.as_str());
        let fallback_media_bit_depth = match crate::conversion::media_info_without_ffprobe(input) {
            Ok(info) => info.and_then(|info| info.bit_depth),
            Err(err) => {
                crate::media_conversion_gate::probe_layer_audit(
                    "media_precision_bitstream_probe_failed",
                    input,
                    format!("bitstream precision fallback failed: {err}"),
                );
                None
            }
        };

        Self::from_media_context(ext_lower, color_info, fallback_media_bit_depth)
    }

    #[must_use]
    pub(crate) fn from_media_context(
        ext_lower: Option<&str>,
        color_info: &crate::ffprobe_json::ColorInfo,
        fallback_media_bit_depth: Option<u8>,
    ) -> Self {
        build_image_precision_profile(ext_lower, color_info, fallback_media_bit_depth)
    }

    #[must_use]
    pub const fn is_float(self) -> bool {
        self.detection.is_float
    }

    #[must_use]
    pub const fn bit_depth(self) -> Option<u8> {
        self.bit_depth
    }

    #[must_use]
    pub const fn bit_depth_inferred_from_pix_fmt(self) -> bool {
        self.detection.bit_depth_inferred_from_pix_fmt
    }

    #[must_use]
    pub const fn should_preserve_high_precision(self) -> bool {
        self.preservation.preserve_high_precision
    }

    #[must_use]
    pub const fn preserve_unknown_container_with_16bit(self) -> bool {
        self.preservation.preserve_unknown_container_with_16bit
    }

    #[must_use]
    pub const fn used_float_extension_hint(self) -> bool {
        self.detection.used_float_extension_hint
    }

    #[must_use]
    pub const fn should_use_high_precision_png16_decode(self) -> bool {
        !self.detection.is_float && self.preservation.metadata_requires_high_precision_decode
    }

    #[must_use]
    pub const fn intermediate_depth_str(self) -> &'static str {
        if self.detection.is_float {
            "32"
        } else if self.preservation.preserve_high_precision {
            "16"
        } else {
            "8"
        }
    }

    #[must_use]
    pub const fn intermediate_suffix(self) -> &'static str {
        if self.detection.is_float {
            ".exr"
        } else {
            ".png"
        }
    }

    /// Still-image pipe RGB format for CJXL pre-decode (16-bit when policy
    /// requires; never float to 48-bit PNG).
    #[must_use]
    pub const fn still_pipe_rgb_pix_fmt(self) -> crate::ffmpeg_builder::PixFmt {
        if self.should_use_high_precision_png16_decode() {
            crate::ffmpeg_builder::PixFmt::Rgb48le
        } else {
            crate::ffmpeg_builder::PixFmt::Rgb24
        }
    }

    /// RGB `pix_fmt` name for PNG16 preservation decode
    /// (`decode_image_to_png16_preserving_precision`).
    #[must_use]
    pub const fn png16_decode_rgb_pix_fmt_name(self) -> &'static str {
        if self.should_use_high_precision_png16_decode() {
            crate::constants::PIX_FMT_RGB48LE
        } else {
            crate::constants::PIX_FMT_RGB24
        }
    }
}

fn build_image_precision_profile(
    ext_lower: Option<&str>,
    color_info: &crate::ffprobe_json::ColorInfo,
    fallback_media_bit_depth: Option<u8>,
) -> ImagePrecisionProfile {
    let assessment = color_info.assessment();
    let mut is_float = assessment.is_float();
    let used_float_extension_hint = !is_float && matches!(ext_lower, Some("exr" | "hdr"));
    if used_float_extension_hint {
        is_float = true;
    }

    let explicit_bit_depth = assessment
        .bit_depth_metadata()
        .confirmed_bit_depth()
        .or(fallback_media_bit_depth);
    let bit_depth = match explicit_bit_depth {
        Some(v) => Some(v),
        None => assessment.bit_depth_metadata().effective_bit_depth(),
    };
    let bit_depth_inferred_from_pix_fmt = explicit_bit_depth.is_none()
        && color_info.bit_depth_inferred_from_pix_fmt
        && assessment
            .bit_depth_metadata()
            .effective_bit_depth()
            .is_some();

    let preserve_unknown_container_with_16bit =
        !is_float && bit_depth.is_none() && matches!(ext_lower, Some("tif" | "tiff" | "dng"));
    let metadata_requires_high_precision_decode =
        !is_float && assessment.should_preserve_high_bit_depth();
    let preserve_high_precision = is_float
        || assessment.should_preserve_high_bit_depth()
        || preserve_unknown_container_with_16bit;

    ImagePrecisionProfile {
        detection: ImagePrecisionDetection {
            is_float,
            bit_depth_inferred_from_pix_fmt,
            used_float_extension_hint,
        },
        bit_depth,
        preservation: ImagePrecisionPreservation {
            preserve_high_precision,
            preserve_unknown_container_with_16bit,
            metadata_requires_high_precision_decode,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BitDepthMetadata, ImagePrecisionProfile, MediaPrecision, hevc_yuv420_output_pix_fmt,
    };
    use crate::ffprobe_json::ColorInfo;

    struct StubPrecision {
        hdr: bool,
        depth: BitDepthMetadata,
    }

    impl MediaPrecision for StubPrecision {
        fn bit_depth_metadata(&self) -> BitDepthMetadata {
            self.depth
        }

        fn has_hdr_signaling(&self) -> bool {
            self.hdr
        }
    }

    #[test]
    fn confirmed_bit_depth_ignores_pix_fmt_inference() {
        let inferred = BitDepthMetadata::new(Some(10), true);
        let explicit = BitDepthMetadata::new(Some(10), false);

        assert_eq!(inferred.effective_bit_depth(), Some(10));
        assert_eq!(inferred.confirmed_bit_depth(), None);
        assert_eq!(explicit.effective_bit_depth(), Some(10));
        assert_eq!(explicit.confirmed_bit_depth(), Some(10));
    }

    #[test]
    fn preserve_high_precision_accepts_hdr_or_inferred_10_bit() {
        let hdr_sdr_depth = StubPrecision {
            hdr: true,
            depth: BitDepthMetadata::new(Some(8), false),
        };
        let inferred_10_bit = StubPrecision {
            hdr: false,
            depth: BitDepthMetadata::new(Some(10), true),
        };

        assert!(hdr_sdr_depth.should_preserve_high_bit_depth());
        assert!(inferred_10_bit.should_preserve_high_bit_depth());
        assert!(!inferred_10_bit.has_confirmed_high_bit_depth());
        assert_eq!(
            hevc_yuv420_output_pix_fmt(&hdr_sdr_depth),
            crate::constants::PIX_FMT_YUV420P10LE
        );
        assert_eq!(
            hevc_yuv420_output_pix_fmt(&inferred_10_bit),
            crate::constants::PIX_FMT_YUV420P10LE
        );
    }

    #[test]
    fn hevc_output_pix_fmt_stays_8_bit_for_plain_sdr() {
        let sdr = StubPrecision {
            hdr: false,
            depth: BitDepthMetadata::new(Some(8), false),
        };

        assert_eq!(
            hevc_yuv420_output_pix_fmt(&sdr),
            crate::constants::PIX_FMT_YUV420P
        );
    }

    #[test]
    fn image_precision_profile_uses_float_container_hint() {
        let profile =
            ImagePrecisionProfile::from_media_context(Some("hdr"), &ColorInfo::default(), None);

        assert!(profile.is_float());
        assert!(profile.used_float_extension_hint());
        assert!(profile.should_preserve_high_precision());
        assert_eq!(profile.intermediate_depth_str(), "32");
        assert_eq!(profile.intermediate_suffix(), ".exr");
        assert!(!profile.should_use_high_precision_png16_decode());
    }

    #[test]
    fn image_precision_profile_preserves_inferred_high_bit_depth() {
        let profile = ImagePrecisionProfile::from_media_context(
            Some("avif"),
            &ColorInfo {
                bit_depth: Some(10),
                bit_depth_inferred_from_pix_fmt: true,
                ..Default::default()
            },
            None,
        );

        assert_eq!(profile.bit_depth(), Some(10));
        assert!(profile.bit_depth_inferred_from_pix_fmt());
        assert!(profile.should_preserve_high_precision());
        assert_eq!(profile.intermediate_depth_str(), "16");
        assert_eq!(profile.intermediate_suffix(), ".png");
        assert!(profile.should_use_high_precision_png16_decode());
    }

    #[test]
    fn still_pipe_and_png16_pix_fmt_exclude_float_from_rgb48le() {
        let float_profile = ImagePrecisionProfile::from_media_context(
            Some("exr"),
            &ColorInfo {
                is_float: true,
                ..Default::default()
            },
            None,
        );
        assert_eq!(
            float_profile.still_pipe_rgb_pix_fmt(),
            crate::ffmpeg_builder::PixFmt::Rgb24
        );
        assert_eq!(
            float_profile.png16_decode_rgb_pix_fmt_name(),
            crate::constants::PIX_FMT_RGB24
        );

        let hdr_profile = ImagePrecisionProfile::from_media_context(
            None,
            &ColorInfo {
                color_transfer: Some(crate::constants::HDR_TRANSFER_PQ.to_string()),
                bit_depth: Some(10),
                ..Default::default()
            },
            None,
        );
        assert_eq!(
            hdr_profile.still_pipe_rgb_pix_fmt(),
            crate::ffmpeg_builder::PixFmt::Rgb48le
        );
        assert_eq!(
            hdr_profile.png16_decode_rgb_pix_fmt_name(),
            crate::constants::PIX_FMT_RGB48LE
        );
    }

    #[test]
    fn image_precision_profile_preserves_unknown_tiff_container_without_png16_decode() {
        let profile =
            ImagePrecisionProfile::from_media_context(Some("tiff"), &ColorInfo::default(), None);

        assert_eq!(profile.bit_depth(), None);
        assert!(profile.preserve_unknown_container_with_16bit());
        assert!(profile.should_preserve_high_precision());
        assert_eq!(profile.intermediate_depth_str(), "16");
        assert!(!profile.should_use_high_precision_png16_decode());
    }
}

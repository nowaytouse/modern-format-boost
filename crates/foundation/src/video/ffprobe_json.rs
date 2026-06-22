//! `FFprobe` JSON Parsing Module
//! Uses `serde_json` instead of manual string parsing

use crate::builder_base::ToolBuilder;
use crate::media_precision::{BitDepthMetadata, MediaPrecision};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeSideData {
    pub side_data_type: Option<String>,
    // mastering display fields
    pub green_x: Option<serde_json::Value>,
    pub green_y: Option<serde_json::Value>,
    pub blue_x: Option<serde_json::Value>,
    pub blue_y: Option<serde_json::Value>,
    pub red_x: Option<serde_json::Value>,
    pub red_y: Option<serde_json::Value>,
    pub white_point_x: Option<serde_json::Value>,
    pub white_point_y: Option<serde_json::Value>,
    pub max_luminance: Option<serde_json::Value>,
    pub min_luminance: Option<serde_json::Value>,
    // CLL fields
    pub max_content: Option<u64>,
    pub max_average: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeStream {
    #[serde(default)]
    pub color_space: Option<String>,
    #[serde(default)]
    pub color_transfer: Option<String>,
    #[serde(default)]
    pub color_primaries: Option<String>,
    #[serde(default)]
    pub color_range: Option<String>,
    #[serde(default)]
    pub pix_fmt: Option<String>,
    #[serde(default)]
    pub bits_per_raw_sample: Option<String>,
    #[serde(default)]
    pub bits_per_sample: Option<String>,
    #[serde(default)]
    pub side_data_list: Vec<FfprobeSideData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeFrame {
    #[serde(default)]
    pub side_data_list: Vec<FfprobeSideData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FfprobeOutput {
    #[serde(default)]
    pub streams: Vec<FfprobeStream>,
    #[serde(default)]
    pub frames: Vec<FfprobeFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrSignalKind {
    DolbyVision,
    Hdr10Plus,
    Hdr10StaticMetadata,
    PqTransfer,
    HlgTransfer,
}

impl HdrSignalKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::DolbyVision => "Dolby Vision",
            Self::Hdr10Plus => "HDR10+",
            Self::Hdr10StaticMetadata => "HDR10 static metadata",
            Self::PqTransfer => "PQ transfer",
            Self::HlgTransfer => "HLG transfer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorInfoAssessment {
    hdr_signal: Option<HdrSignalKind>,
    bit_depth: BitDepthMetadata,
    is_float: bool,
    has_wide_gamut_signal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ColorProbeFlags {
    pub has_mastering_display: bool,
    pub has_max_cll: bool,
    pub is_dolby_vision: bool,
    pub is_hdr10_plus: bool,
    pub is_float: bool,
}

impl ColorInfoAssessment {
    #[must_use]
    pub fn from_probe_fields(
        color_space: Option<&str>,
        color_transfer: Option<&str>,
        color_primaries: Option<&str>,
        bit_depth: BitDepthMetadata,
        flags: ColorProbeFlags,
    ) -> Self {
        let color_space_lower = color_space.map(str::to_ascii_lowercase);
        let color_transfer_lower = color_transfer.map(str::to_ascii_lowercase);
        let color_primaries_lower = color_primaries.map(str::to_ascii_lowercase);

        let hdr_signal = if flags.is_dolby_vision {
            Some(HdrSignalKind::DolbyVision)
        } else if flags.is_hdr10_plus {
            Some(HdrSignalKind::Hdr10Plus)
        } else if flags.has_mastering_display || flags.has_max_cll {
            Some(HdrSignalKind::Hdr10StaticMetadata)
        } else if color_transfer_lower.as_deref() == Some(crate::constants::HDR_TRANSFER_PQ) {
            Some(HdrSignalKind::PqTransfer)
        } else if color_transfer_lower.as_deref() == Some(crate::constants::HDR_TRANSFER_HLG) {
            Some(HdrSignalKind::HlgTransfer)
        } else {
            None
        };

        let has_wide_gamut_signal = matches!(
            color_primaries_lower.as_deref(),
            Some("bt2020" | "display-p3" | "smpte432" | "adobergb")
        ) || matches!(
            color_space_lower.as_deref(),
            Some("bt2020" | "bt2020nc" | "bt2020ncl" | "adobergb")
        );

        Self {
            hdr_signal,
            bit_depth,
            is_float: flags.is_float,
            has_wide_gamut_signal,
        }
    }

    #[must_use]
    pub const fn hdr_signal(self) -> Option<HdrSignalKind> {
        self.hdr_signal
    }

    #[must_use]
    pub fn hdr_signal_label(self) -> Option<&'static str> {
        self.hdr_signal.map(HdrSignalKind::label)
    }

    #[must_use]
    pub const fn bit_depth_metadata(self) -> BitDepthMetadata {
        self.bit_depth
    }

    #[must_use]
    pub const fn is_float(self) -> bool {
        self.is_float
    }

    #[must_use]
    pub const fn has_hdr_signaling(self) -> bool {
        self.hdr_signal.is_some()
    }

    #[must_use]
    pub const fn has_wide_gamut_signal(self) -> bool {
        self.has_wide_gamut_signal
    }

    #[must_use]
    pub fn should_preserve_high_bit_depth(self) -> bool {
        self.has_hdr_signaling() || self.bit_depth.has_high_bit_depth()
    }

    #[must_use]
    pub fn has_confirmed_high_bit_depth(self) -> bool {
        self.bit_depth.has_confirmed_high_bit_depth()
    }

    #[must_use]
    pub fn needs_high_precision_png_decode(self) -> bool {
        !self.is_float && self.should_preserve_high_bit_depth()
    }

    #[must_use]
    pub fn should_carry_conversion_metadata(self) -> bool {
        self.is_float || self.has_wide_gamut_signal || self.should_preserve_high_bit_depth()
    }
}

/// Raw ffprobe-derived color probe data.
///
/// This struct stores extracted fields and low-level probe flags only. Semantic
/// interpretation such as "is this true HDR?" or "should conversion preserve
/// high precision?" must flow through `ColorInfoAssessment`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct ColorInfo {
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub color_range: Option<String>,
    pub pix_fmt: Option<String>,
    pub bit_depth: Option<u8>,
    pub bit_depth_inferred_from_pix_fmt: bool,
    /// HDR10 mastering display string (ffmpeg format)
    pub mastering_display: Option<String>,
    /// HDR10 CLL: "MaxCLL,MaxFALL"
    pub max_cll: Option<String>,
    pub is_dolby_vision: bool,
    pub is_hdr10_plus: bool,
    pub is_float: bool,
}

#[must_use]
pub fn pix_fmt_indicates_float(pix_fmt: Option<&str>) -> bool {
    let Some(pix_fmt) = pix_fmt else {
        return false;
    };

    let pf_lower = pix_fmt.to_ascii_lowercase();
    pf_lower.contains('f')
        && (pf_lower.contains("32") || pf_lower.contains("16") || pf_lower.contains("64"))
        && (pf_lower.contains("pf32")
            || pf_lower.contains("f32")
            || pf_lower.contains("pf16")
            || pf_lower.contains("f16"))
}

impl ColorInfo {
    #[must_use]
    pub fn assessment(&self) -> ColorInfoAssessment {
        ColorInfoAssessment::from_probe_fields(
            self.color_space.as_deref(),
            self.color_transfer.as_deref(),
            self.color_primaries.as_deref(),
            BitDepthMetadata::new(self.bit_depth, self.bit_depth_inferred_from_pix_fmt),
            ColorProbeFlags {
                has_mastering_display: self.mastering_display.is_some(),
                has_max_cll: self.max_cll.is_some(),
                is_dolby_vision: self.is_dolby_vision,
                is_hdr10_plus: self.is_hdr10_plus,
                is_float: self.is_float,
            },
        )
    }

    /// Returns true when the content is any form of HDR (PQ, HLG, DV, HDR10, HDR10+)
    #[must_use]
    pub fn is_hdr(&self) -> bool {
        self.assessment().has_hdr_signaling()
    }

    #[must_use]
    pub fn needs_high_precision_png_decode(&self) -> bool {
        self.assessment().needs_high_precision_png_decode()
    }

    #[must_use]
    pub fn png_decode_rgb_pix_fmt(&self) -> &'static str {
        crate::media_precision::ImagePrecisionProfile::from_media_context(None, self, None)
            .png16_decode_rgb_pix_fmt_name()
    }
}

impl MediaPrecision for ColorInfo {
    fn bit_depth_metadata(&self) -> BitDepthMetadata {
        self.assessment().bit_depth_metadata()
    }

    fn has_hdr_signaling(&self) -> bool {
        self.assessment().has_hdr_signaling()
    }
}

fn parse_stream_bit_depth(stream: &FfprobeStream) -> (Option<u8>, bool) {
    let explicit = crate::media_conversion_gate::probe_ffprobe_stream_bit_depth_u8_from_fields(
        stream.bits_per_raw_sample.as_deref(),
        stream.bits_per_sample.as_deref(),
    );

    if explicit.is_some() {
        return (explicit, false);
    }

    if let Some(inferred) = stream
        .pix_fmt
        .as_deref()
        .and_then(crate::ffprobe::detect_bit_depth)
    {
        return (Some(inferred), true);
    }

    (None, false)
}

fn rational_to_50k(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::String(s) => {
            if let Some((n, d)) = s.split_once('/') {
                let n: f64 = crate::numeric_cast::parse_strict(n, "hdr_gx_num")?;
                let d: f64 = crate::numeric_cast::parse_strict(d, "hdr_gx_den")?;
                if crate::numeric_cast::is_effectively_zero(
                    d,
                    crate::numeric_cast::FloatContext::FfmpegMeasurement,
                ) {
                    return None;
                }
                crate::numeric_cast::f64_to_u64_strict(
                    ((n / d) * crate::constants::HDR_COORD_SCALING_FACTOR).round(),
                    "hdr_coord",
                )
            } else {
                let f: f64 = crate::numeric_cast::parse_strict(s, "hdr_gx_val")?;
                crate::numeric_cast::f64_to_u64_strict(
                    (f * crate::constants::HDR_COORD_SCALING_FACTOR).round(),
                    "hdr_coord",
                )
            }
        }
        _ => None,
    }
}

fn rational_to_10k(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::String(s) => {
            if let Some((n, d)) = s.split_once('/') {
                let n: f64 = crate::numeric_cast::parse_strict(n, "hdr_lmax_num")?;
                let d: f64 = crate::numeric_cast::parse_strict(d, "hdr_lmax_den")?;
                if crate::numeric_cast::is_effectively_zero(
                    d,
                    crate::numeric_cast::FloatContext::FfmpegMeasurement,
                ) {
                    return None;
                }
                crate::numeric_cast::f64_to_u64_strict(
                    ((n / d) * crate::constants::HDR_LUMA_SCALING_FACTOR).round(),
                    "hdr_luma",
                )
            } else {
                let f: f64 = crate::numeric_cast::parse_strict(s, "hdr_lmax_val")?;
                crate::numeric_cast::f64_to_u64_strict(
                    (f * crate::constants::HDR_LUMA_SCALING_FACTOR).round(),
                    "hdr_luma_f",
                )
            }
        }
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            crate::numeric_cast::f64_to_u64_strict(
                (f * crate::constants::HDR_LUMA_SCALING_FACTOR).round(),
                "hdr_luma_n",
            )
        }
        _ => None,
    }
}

fn parse_side_data_list(
    entries: &[FfprobeSideData],
    is_dolby_vision: &mut bool,
    is_hdr10_plus: &mut bool,
    mastering_display: &mut Option<String>,
    max_cll: &mut Option<String>,
) {
    for sd in entries {
        let sd_type =
            crate::media_conversion_gate::probe_side_data_type_label(sd.side_data_type.as_deref());

        if sd_type.contains("dolby vision") || sd_type.contains("dovi") {
            *is_dolby_vision = true;
        }
        if sd_type.contains("hdr dynamic")
            || sd_type.contains("st2094")
            || sd_type.contains("hdr10+")
        {
            *is_hdr10_plus = true;
        }
        if sd_type.contains("mastering display")
            && mastering_display.is_none()
            && let (
                Some(gx),
                Some(gy),
                Some(bx),
                Some(by_),
                Some(rx),
                Some(ry),
                Some(wx),
                Some(wy),
                Some(lmax),
                Some(lmin),
            ) = (
                sd.green_x.as_ref().and_then(rational_to_50k),
                sd.green_y.as_ref().and_then(rational_to_50k),
                sd.blue_x.as_ref().and_then(rational_to_50k),
                sd.blue_y.as_ref().and_then(rational_to_50k),
                sd.red_x.as_ref().and_then(rational_to_50k),
                sd.red_y.as_ref().and_then(rational_to_50k),
                sd.white_point_x.as_ref().and_then(rational_to_50k),
                sd.white_point_y.as_ref().and_then(rational_to_50k),
                sd.max_luminance.as_ref().and_then(rational_to_10k),
                sd.min_luminance.as_ref().and_then(rational_to_10k),
            )
        {
            *mastering_display = Some(format!(
                "G({gx},{gy})B({bx},{by_})R({rx},{ry})WP({wx},{wy})L({lmax},{lmin})"
            ));
        }
        if sd_type.contains("content light level")
            && max_cll.is_none()
            && let (Some(mc), Some(ma)) = (sd.max_content, sd.max_average)
        {
            *max_cll = Some(format!("{mc},{ma}"));
        }
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[must_use]
pub fn extract_color_info(input: &Path) -> ColorInfo {
    let input_str = input.to_string_lossy();

    let output = match crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(input)
        .loglevel("error")
        .print_format("json")
        .show_streams()
        .show_frames()
        .read_intervals("%+#5")
        .select_stream(crate::ffmpeg_builder::StreamType::Video, 0)
        .build()
        .output()
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            // Check if failure is due to image2 demuxer pattern matching (e.g., filenames with [])
            let stderr = String::from_utf8_lossy(&o.stderr);
            if stderr.contains("Could find no file with path")
                && stderr.contains("and index in the range")
            {
                crate::log_rare_error!(
                    "FFprobe",
                    "Image2 demuxer pattern matching failed for file: {} - Retrying with -pattern_type none",
                    input_str
                );
                // Retry with -pattern_type none to disable sequence pattern matching
                match crate::ffmpeg_builder::FfprobeBuilder::new()
                    .input(input)
                    .loglevel("error")
                    .pattern_type("none")
                    .print_format("json")
                    .show_streams()
                    .show_frames()
                    .read_intervals("%+#5")
                    .select_stream(crate::ffmpeg_builder::StreamType::Video, 0)
                    .build()
                    .output()
                {
                    Ok(retry_o) if retry_o.status.success() => retry_o,
                    Ok(retry_o) => {
                        let retry_stderr = String::from_utf8_lossy(&retry_o.stderr);
                        crate::media_conversion_gate::probe_ffprobe_input_audit(
                            "ffprobe_color_pattern_fallback_failed",
                            &input_str,
                            format!(
                                "pattern_type fallback non-zero exit: {}",
                                retry_stderr.trim()
                            ),
                        );
                        return ColorInfo::default();
                    }
                    Err(err) => {
                        crate::media_conversion_gate::probe_ffprobe_input_audit(
                            "ffprobe_color_pattern_fallback_launch_failed",
                            &input_str,
                            format!("pattern_type fallback failed to start: {err}"),
                        );
                        crate::log_rare_error!(
                            "FFprobe",
                            "Pattern_type fallback also failed for: {}",
                            input_str
                        );
                        return ColorInfo::default();
                    }
                }
            } else {
                // For JPEG/image files, ffprobe failure is often expected (not a video stream)
                // Only log as warning if stderr suggests a real error (not just "no video stream")
                let stderr_lower = stderr.to_lowercase();
                if !stderr_lower.contains("no such file")
                    && !stderr_lower.contains("invalid data")
                    && !stderr_lower.is_empty()
                {
                    crate::media_conversion_gate::probe_ffprobe_input_audit(
                        "ffprobe_color_query_failed",
                        &input_str,
                        format!("ffprobe execution failed: {}", stderr.trim()),
                    );
                }
                return ColorInfo::default();
            }
        }
        Err(e) => {
            crate::media_conversion_gate::probe_ffprobe_input_audit(
                "ffprobe_color_launch_failed",
                &input_str,
                format!("failed to launch ffprobe subprocess: {e}"),
            );
            return ColorInfo::default();
        }
    };

    let json_str = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(e) => {
            crate::media_conversion_gate::probe_ffprobe_input_audit(
                "ffprobe_color_stdout_invalid_utf8",
                &input_str,
                format!("ffprobe stdout invalid UTF-8: {e}"),
            );
            return ColorInfo::default();
        }
    };

    let parsed: FfprobeOutput = match serde_json::from_str(&json_str) {
        Ok(p) => p,
        Err(e) => {
            crate::media_conversion_gate::probe_ffprobe_input_audit(
                "ffprobe_color_json_parse_failed",
                &input_str,
                format!("ffprobe JSON parse failed: {e}"),
            );
            return ColorInfo::default();
        }
    };

    let Some(stream) = parsed.streams.first() else {
        crate::media_conversion_gate::probe_ffprobe_input_audit(
            "ffprobe_color_no_video_stream",
            &input_str,
            "no valid video streams in ffprobe output",
        );
        return ColorInfo::default();
    };

    let (bit_depth, bit_depth_inferred_from_pix_fmt) = parse_stream_bit_depth(stream);

    let color_space = stream
        .color_space
        .clone()
        .filter(|s| !s.is_empty() && s != "unknown");
    let color_transfer = stream
        .color_transfer
        .clone()
        .filter(|s| !s.is_empty() && s != "unknown");
    let color_primaries = stream
        .color_primaries
        .clone()
        .filter(|s| !s.is_empty() && s != "unknown");
    let color_range = stream
        .color_range
        .clone()
        .filter(|s| !s.is_empty() && s != "unknown");

    let mut is_dolby_vision = false;
    let mut is_hdr10_plus = false;
    let mut mastering_display: Option<String> = None;
    let mut max_cll: Option<String> = None;

    parse_side_data_list(
        &stream.side_data_list,
        &mut is_dolby_vision,
        &mut is_hdr10_plus,
        &mut mastering_display,
        &mut max_cll,
    );

    for frame in &parsed.frames {
        parse_side_data_list(
            &frame.side_data_list,
            &mut is_dolby_vision,
            &mut is_hdr10_plus,
            &mut mastering_display,
            &mut max_cll,
        );
    }

    let is_float = pix_fmt_indicates_float(stream.pix_fmt.as_deref());

    ColorInfo {
        color_space,
        color_transfer,
        color_primaries,
        color_range,
        pix_fmt: stream.pix_fmt.clone(),
        bit_depth,
        bit_depth_inferred_from_pix_fmt,
        mastering_display,
        max_cll,
        is_dolby_vision,
        is_hdr10_plus,
        is_float,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_json() {
        let json = r#"{"streams":[{"color_space":"bt709","pix_fmt":"yuv420p","bits_per_raw_sample":"8"}]}"#;
        let parsed: FfprobeOutput =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(parsed.streams.len(), 1);
        assert_eq!(
            parsed.streams.first().and_then(|s| s.color_space.clone()),
            Some("bt709".to_string())
        );
        assert_eq!(
            parsed.streams.first().and_then(|s| s.pix_fmt.clone()),
            Some("yuv420p".to_string())
        );
    }

    #[test]
    fn test_parse_empty_streams() {
        let json = r#"{"streams":[]}"#;
        let parsed: FfprobeOutput =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert!(parsed.streams.is_empty());
    }

    #[test]
    fn test_parse_missing_fields() {
        let json = r#"{"streams":[{"pix_fmt":"yuv420p10le"}]}"#;
        let parsed: FfprobeOutput =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(
            parsed.streams.first().and_then(|s| s.color_space.clone()),
            None
        );
        assert_eq!(
            parsed.streams.first().and_then(|s| s.pix_fmt.clone()),
            Some("yuv420p10le".to_string())
        );
    }

    #[test]
    fn test_is_hdr_pq() {
        let ci = ColorInfo {
            color_transfer: Some("smpte2084".to_string()),
            ..Default::default()
        };
        assert!(ci.is_hdr());
    }

    #[test]
    fn test_is_hdr_hlg() {
        let ci = ColorInfo {
            color_transfer: Some("arib-std-b67".to_string()),
            ..Default::default()
        };
        assert!(ci.is_hdr());
    }

    #[test]
    fn test_not_hdr_sdr() {
        let ci = ColorInfo {
            color_space: Some("bt709".to_string()),
            color_transfer: Some("bt709".to_string()),
            ..Default::default()
        };
        assert!(!ci.is_hdr());
    }

    #[test]
    fn test_color_info_assessment_prefers_explicit_hdr_signal_over_transfer_inference() {
        let ci = ColorInfo {
            color_transfer: Some("smpte2084".to_string()),
            is_dolby_vision: true,
            ..Default::default()
        };

        let assessment = ci.assessment();
        assert_eq!(assessment.hdr_signal(), Some(HdrSignalKind::DolbyVision));
        assert_eq!(assessment.hdr_signal_label(), Some("Dolby Vision"));
    }

    #[test]
    fn test_color_info_assessment_marks_bt2020_as_conversion_relevant_without_hdr() {
        let ci = ColorInfo {
            color_primaries: Some("bt2020".to_string()),
            color_space: Some("bt2020nc".to_string()),
            ..Default::default()
        };

        let assessment = ci.assessment();
        assert!(!assessment.has_hdr_signaling());
        assert!(assessment.has_wide_gamut_signal());
        assert!(assessment.should_carry_conversion_metadata());
    }

    #[test]
    fn test_color_info_assessment_treats_adobergb_as_wide_gamut_hint() {
        let ci = ColorInfo {
            color_space: Some("AdobeRGB".to_string()),
            ..Default::default()
        };

        let assessment = ci.assessment();
        assert!(!assessment.has_hdr_signaling());
        assert!(assessment.has_wide_gamut_signal());
        assert!(assessment.should_carry_conversion_metadata());
    }

    #[test]
    fn test_color_info_assessment_treats_empty_probe_data_as_not_conversion_relevant() {
        let assessment = ColorInfo::default().assessment();

        assert!(!assessment.has_hdr_signaling());
        assert!(!assessment.has_wide_gamut_signal());
        assert!(!assessment.should_carry_conversion_metadata());
    }

    #[test]
    fn test_pix_fmt_indicates_float_detects_common_ffmpeg_float_formats() {
        assert!(pix_fmt_indicates_float(Some("gbrpf32le")));
        assert!(pix_fmt_indicates_float(Some("rgbaf16le")));
        assert!(!pix_fmt_indicates_float(Some("yuv420p10le")));
        assert!(!pix_fmt_indicates_float(None));
    }

    #[test]
    fn test_parse_stream_bit_depth_prefers_explicit_raw_sample() {
        let stream = FfprobeStream {
            bits_per_raw_sample: Some("12".to_string()),
            bits_per_sample: Some("10".to_string()),
            pix_fmt: Some("yuv420p8le".to_string()),
            ..Default::default()
        };

        assert_eq!(parse_stream_bit_depth(&stream), (Some(12), false));
    }

    #[test]
    fn test_parse_stream_bit_depth_falls_back_to_bits_per_sample_then_pix_fmt() {
        let sample_stream = FfprobeStream {
            bits_per_sample: Some("10".to_string()),
            pix_fmt: Some("yuv420p".to_string()),
            ..Default::default()
        };
        assert_eq!(parse_stream_bit_depth(&sample_stream), (Some(10), false));

        let pix_fmt_stream = FfprobeStream {
            pix_fmt: Some("yuv420p10le".to_string()),
            ..Default::default()
        };
        assert_eq!(parse_stream_bit_depth(&pix_fmt_stream), (Some(10), true));
    }

    #[test]
    fn test_confirmed_bit_depth_ignores_pix_fmt_inference() {
        let inferred = ColorInfo {
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: true,
            ..Default::default()
        };
        assert_eq!(inferred.effective_bit_depth(), Some(10));
        assert_eq!(inferred.confirmed_bit_depth(), None);
        assert!(inferred.should_preserve_high_bit_depth());
        assert!(!inferred.has_confirmed_high_bit_depth());

        let explicit = ColorInfo {
            bit_depth: Some(10),
            bit_depth_inferred_from_pix_fmt: false,
            ..Default::default()
        };
        assert_eq!(explicit.effective_bit_depth(), Some(10));
        assert_eq!(explicit.confirmed_bit_depth(), Some(10));
        assert!(explicit.should_preserve_high_bit_depth());
        assert!(explicit.has_confirmed_high_bit_depth());
    }

    #[test]
    fn test_parse_side_data_list_detects_hdr10_plus_from_stream_entries() {
        let parsed: FfprobeOutput = serde_json::from_str(
            r#"{
                "streams": [{
                    "pix_fmt": "yuv420p10le",
                    "side_data_list": [{
                        "side_data_type": "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
                    }]
                }]
            }"#,
        )
        .unwrap_or_else(|e| panic!("error: {e:?}"));

        let stream = parsed.streams.first().expect("expected one stream");
        let (bit_depth, bit_depth_inferred_from_pix_fmt) = parse_stream_bit_depth(stream);
        let mut is_dolby_vision = false;
        let mut is_hdr10_plus = false;
        let mut mastering_display = None;
        let mut max_cll = None;
        parse_side_data_list(
            &stream.side_data_list,
            &mut is_dolby_vision,
            &mut is_hdr10_plus,
            &mut mastering_display,
            &mut max_cll,
        );

        let ci = ColorInfo {
            pix_fmt: stream.pix_fmt.clone(),
            bit_depth,
            bit_depth_inferred_from_pix_fmt,
            is_dolby_vision,
            is_hdr10_plus,
            mastering_display,
            max_cll,
            ..Default::default()
        };

        assert!(ci.is_hdr());
        assert_eq!(ci.assessment().hdr_signal(), Some(HdrSignalKind::Hdr10Plus));
    }

    #[test]
    fn test_parse_side_data_list_detects_hdr10_plus_from_frame_entries() {
        let parsed: FfprobeOutput = serde_json::from_str(
            r#"{
                "streams": [{
                    "pix_fmt": "yuv420p10le"
                }],
                "frames": [{
                    "side_data_list": [{
                        "side_data_type": "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
                    }]
                }]
            }"#,
        )
        .unwrap_or_else(|e| panic!("error: {e:?}"));

        let stream = parsed.streams.first().expect("expected one stream");
        let (bit_depth, bit_depth_inferred_from_pix_fmt) = parse_stream_bit_depth(stream);
        let mut is_dolby_vision = false;
        let mut is_hdr10_plus = false;
        let mut mastering_display = None;
        let mut max_cll = None;

        parse_side_data_list(
            &stream.side_data_list,
            &mut is_dolby_vision,
            &mut is_hdr10_plus,
            &mut mastering_display,
            &mut max_cll,
        );
        for frame in &parsed.frames {
            parse_side_data_list(
                &frame.side_data_list,
                &mut is_dolby_vision,
                &mut is_hdr10_plus,
                &mut mastering_display,
                &mut max_cll,
            );
        }

        let ci = ColorInfo {
            pix_fmt: stream.pix_fmt.clone(),
            bit_depth,
            bit_depth_inferred_from_pix_fmt,
            is_dolby_vision,
            is_hdr10_plus,
            mastering_display,
            max_cll,
            ..Default::default()
        };

        assert!(ci.is_hdr());
        assert_eq!(ci.assessment().hdr_signal(), Some(HdrSignalKind::Hdr10Plus));
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_json_parse_roundtrip(
            cs in "[a-z0-9]{1,10}",
            pf in "[a-z0-9]{1,15}",
            bd in 8u8..=16
        ) {
            let json = format!(
                r#"{{"streams":[{{"color_space":"{cs}","pix_fmt":"{pf}","bits_per_raw_sample":"{bd}"}}]}}"#
            );
            let parsed: Result<FfprobeOutput, _> = serde_json::from_str(&json);
            prop_assert!(parsed.is_ok());
            let p = parsed.unwrap_or_else(|e| panic!("error: {e:?}"));
            prop_assert_eq!(p.streams.first().and_then(|s| s.color_space.clone()), Some(cs));
            prop_assert_eq!(p.streams.first().and_then(|s| s.pix_fmt.clone()), Some(pf));
        }

        #[test]
        fn prop_invalid_json_no_panic(s in ".*") {
            let _ = serde_json::from_str::<FfprobeOutput>(&s);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtsIntegrity {
    Healthy,
    Duplicate,
    Broken,
}

pub fn check_pts_integrity(input: &Path) -> anyhow::Result<PtsIntegrity> {
    let output = match crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(input)
        .loglevel("error")
        .select_stream(crate::ffmpeg_builder::StreamType::Video, 0)
        .show_entries("packet=pts_time")
        .print_format("csv=p=0")
        .read_intervals("%+#100")
        .build()
        .output()
    {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            crate::media_conversion_gate::probe_ffprobe_path_audit(
                "ffprobe_pts_integrity_failed",
                input,
                format!(
                    "PTS integrity probe failed: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
            );
            return Err(anyhow::anyhow!(
                "PTS integrity probe failed for {}: {}",
                input.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Err(err) => {
            crate::media_conversion_gate::probe_ffprobe_path_audit(
                "ffprobe_pts_integrity_failed",
                input,
                format!("PTS integrity probe failed to launch: {err}"),
            );
            return Err(anyhow::anyhow!(
                "PTS integrity probe failed to launch for {}: {err}",
                input.display()
            ));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut last_pts: Option<f64> = None;
    let mut has_duplicates = false;
    let mut has_backwards = false;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let pts = trimmed.parse::<f64>().map_err(|err| {
            anyhow::anyhow!(
                "failed to parse PTS value {trimmed:?} for {}: {err}",
                input.display()
            )
        })?;
        if let Some(last) = last_pts {
            // Large epsilon for floating point comparison issues
            if pts < last - 1e-4_f64 {
                has_backwards = true;
                break;
            } else if crate::numeric_cast::is_effectively_equal(
                pts,
                last,
                crate::numeric_cast::FloatContext::FfmpegMeasurement,
            ) {
                has_duplicates = true;
            }
        }
        last_pts = Some(pts);
    }

    Ok(if has_backwards {
        PtsIntegrity::Broken
    } else if has_duplicates {
        PtsIntegrity::Duplicate
    } else {
        PtsIntegrity::Healthy
    })
}

#[cfg(test)]
mod pts_integrity_tests {
    use super::*;

    #[test]
    fn pts_integrity_missing_file_returns_error_not_healthy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.mp4");

        let err = check_pts_integrity(&missing)
            .expect_err("missing PTS integrity target must be an error");

        assert!(err.to_string().contains("missing.mp4"));
    }
}

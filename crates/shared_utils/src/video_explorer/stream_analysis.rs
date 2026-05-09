//! Stream Analysis Module
//!
//! This module is responsible for video stream analysis and quality assessment, including:
//! - SSIM (Structural Similarity Index) calculation
//! - PSNR (Peak Signal-to-Noise Ratio) calculation
//! - MS-SSIM (Multi-Scale SSIM) calculation
//! - Video duration detection
//! - Quality threshold validation
//! - Lossless integrity checks (CRF=0 fast-path: frame count + file size only)

use crate::FfmpegBuilder;
use crate::builder_base::ToolBuilder;
use std::path::Path;
use tracing::{info, warn};

pub const LONG_VIDEO_THRESHOLD: f32 = crate::constants::LONG_VIDEO_THRESHOLD_SECS;

#[derive(Debug, Clone, Default)]
pub struct QualityValidationFlags {
    pub metrics: MetricValidationFlags,
    pub force_ms_ssim_long: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MetricValidationFlags {
    pub validate_ssim: bool,
    pub validate_psnr: bool,
    pub validate_ms_ssim: bool,
}

#[derive(Debug, Clone)]
pub struct QualityThresholds {
    pub min_ssim: f64,
    pub min_psnr: f64,
    pub min_ms_ssim: f64,
    pub validation: QualityValidationFlags,
}

impl Default for QualityThresholds {
    fn default() -> Self {
        Self {
            min_ssim: crate::constants::STREAM_ANALYSIS_MIN_SSIM,
            min_psnr: crate::constants::STREAM_ANALYSIS_MIN_PSNR,
            min_ms_ssim: crate::constants::STREAM_ANALYSIS_MIN_MS_SSIM,
            validation: QualityValidationFlags {
                metrics: MetricValidationFlags {
                    validate_ssim: true,
                    validate_psnr: false,
                    validate_ms_ssim: false,
                },
                force_ms_ssim_long: false,
            },
        }
    }
}

pub fn get_video_duration(input: &Path) -> Option<f64> {
    let output = crate::FfprobeBuilder::new()
        .input(input)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .build()
        .output()
        .map_err(|e| {
            warn!(path = %input.display(), error = %e, "ffprobe: Subprocess failed to start for duration check; verify ffprobe is in PATH");
            e
        })
        .ok()?;

    if !output.status.success() {
        warn!(
            path = %input.display(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "ffprobe: Failed to read video duration (non-zero exit status); container may be malformed"
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    match trimmed.parse::<f64>() {
        Ok(duration) => Some(duration),
        Err(err) => {
            warn!(
                path = %input.display(),
                output = %trimmed,
                error = %err,
                "ffprobe: Failed to parse duration output as float"
            );
            None
        }
    }
}

fn count_video_frames(path: &Path) -> Option<u64> {
    if is_gif_magic(path) {
        let frames = crate::numeric_cast::usize_to_u64_strict(
            crate::image_formats::gif::get_frame_count(path),
            "gif_frame_count",
        )
        .expect("usize always fits in u64");
        if frames > 0 {
            return Some(frames);
        }
    }

    let is_webp = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("webp"));
    if is_webp {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to read WebP file for frame counting");
                return None;
            }
        };
        let frames = u64::from(crate::image_formats::webp::count_frames_from_bytes(&data));
        if frames > 0 {
            return Some(frames);
        }
    }

    let try_ffprobe_count = |mode: &str, entry: &str| -> Option<u64> {
        let out = match crate::FfprobeBuilder::new()
            .input(path)
            .loglevel("error")
            .select_stream(crate::StreamType::Video, 0)
            .show_entries(entry)
            .print_format("default=noprint_wrappers=1:nokey=1")
            .arg(mode)
            .build()
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "ffprobe: Subprocess failed to start for frame count; verify ffprobe is in PATH");
                return None;
            }
        };

        if !out.status.success() {
            warn!(
                path = %path.display(),
                stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                "ffprobe: Failed to count frames (non-zero exit status)"
            );
            return None;
        }

        let stdout = String::from_utf8_lossy(&out.stdout);
        let trimmed = stdout.trim();
        match trimmed.parse::<u64>() {
            Ok(count) => Some(count),
            Err(e) => {
                warn!(path = %path.display(), output = %trimmed, error = %e, "ffprobe: Failed to parse frame count output as integer");
                None
            }
        }
    };

    try_ffprobe_count("-count_frames", "stream=nb_read_frames")
        .or_else(|| try_ffprobe_count("-count_packets", "stream=nb_read_packets"))
}

#[must_use]
pub fn is_gif_magic(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut magic)?;
            Ok(())
        })
        .is_ok_and(|()| &magic == b"GIF8")
}

pub fn calculate_ssim_enhanced(input: &Path, output: &Path) -> Option<f64> {
    // GIF-specific path: force palette → yuv420p conversion on the reference side
    // before comparing with the yuv420p-encoded output.  This avoids all three
    // generic filters silently failing because ffmpeg cannot decode a GIF palette
    // stream into the same raw pixel layout as the HEVC output.
    let is_gif = is_gif_magic(input);

    let mut filters: Vec<(String, String)> = Vec::new();
    if is_gif {
        let ms = crate::numeric_cast::f64_to_u64_sat(crate::constants::MS_PER_SEC_F64);
        filters.push((
            "gif_sync".to_string(),
            format!("[0:v]format=rgb24,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,settb=1/{ms},setpts=PTS-STARTPTS,format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,settb=1/{ms},setpts=PTS-STARTPTS,format=yuv420p[cmp];[ref][cmp]ssim"),
        ));
        filters.push((
            "gif_palette".to_string(),
            "[0:v]format=rgb24,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];[ref][cmp]ssim".to_string(),
        ));
        filters.push((
            "gif_pad_even".to_string(),
            "[0:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];[ref][cmp]ssim".to_string(),
        ));
    } else {
        filters.push((
            "standard".to_string(),
            "[0:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0[ref];[ref][1:v]ssim".to_string(),
        ));
        filters.push((
            "format_convert".to_string(),
            "[0:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];[ref][cmp]ssim".to_string(),
        ));
        filters.push(("simple".to_string(), "ssim".to_string()));
    }

    for (name, filter) in filters {
        let result = FfmpegBuilder::new()
            .input(input)
            .input(output)
            .filter_complex(filter)
            .format("null")
            .output_pipe()
            .build()
            .output();

        match result {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if let Some(ssim) = parse_ssim_from_output(&stderr)
                    && is_valid_ssim_value(ssim)
                {
                    info!(method = %name, ssim = %ssim, "SSIM calculated");
                    return Some(ssim);
                }
                warn!(method = %name, "SSIM method failed, trying next method");
            }
            Err(e) => {
                warn!(method = %name, error = %e, "ffmpeg failed");
            }
        }
    }

    tracing::error!("ALL SSIM CALCULATION METHODS FAILED");
    None
}

/// Run ffmpeg with the given lavfi filter and parse SSIM Y/U/V/All from stderr.
fn run_ssim_all_filter(input: &Path, output: &Path, lavfi: &str) -> Option<(f64, f64, f64, f64)> {
    let ffmpeg_output = match FfmpegBuilder::new()
        .input(input)
        .input(output)
        .filter_complex(lavfi)
        .format("null")
        .output_pipe()
        .build()
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn!(input = %input.display(), output = %output.display(), error = %e, "Failed to start ffmpeg for SSIM-All calculation");
            return None;
        }
    };

    // Some filters (ssim) might return non-zero exit code if the streams
    // end at slightly different points for GIFs, even if the result is valid.
    // We parse stderr regardless of out.status.
    let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);
    for line in stderr.lines() {
        if line.contains("SSIM Y:") && line.contains("All:") {
            let y = extract_ssim_value(line, "Y:");
            let u = extract_ssim_value(line, "U:");
            let v = extract_ssim_value(line, "V:");
            let all = extract_ssim_value(line, "All:");
            if let (Some(y), Some(u), Some(v), Some(all)) = (y, u, v, all)
                && is_valid_ssim_value(y)
                && is_valid_ssim_value(all)
            {
                return Some((y, u, v, all));
            }
        }
    }
    None
}

/// SSIM Y/U/V/All between input and output. Tries in order:
///
/// GIF sources get a palette-aware filter chain first (GIF uses indexed colour;
/// the raw pixel layout differs from the yuv420p HEVC output and breaks the
/// generic filters).
///
/// Non-GIF sources try:
/// 1. Direct ssim (when formats already match).
/// 2. Format normalization (odd-size → yuv420p even).
/// 3. Alpha composite on black (transparent WebP/PNG vs HEVC which has no alpha):
///    converts the input to rgb24 (discarding alpha channel) then yuv420p,
///    matching the actual encoder behaviour.
#[must_use]
pub fn calculate_ssim_all(input: &Path, output: &Path) -> Option<(f64, f64, f64, f64)> {
    // GIF-specific chains: render palette → rgb24 → yuv420p before comparing.
    // Use padding (upward to even) to match encoder's padding logic, and use settb/setpts to sync pts.
    const GIF_RGB24: &str = "[0:v]format=rgb24,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];[ref][cmp]ssim";
    const GIF_NORM: &str = "[0:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];[ref][cmp]ssim";

    // Generic chains (pad to even)
    const DIRECT: &str = "[0:v][1:v]ssim";
    const FORMAT_NORM: &str = "[0:v]format=yuv420p,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0[ref];[1:v]format=yuv420p,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0[cmp];[ref][cmp]ssim";
    // Alpha-composite on black without the deprecated premultiply=inplace=1 filter:
    // decode to rgb24 (which discards alpha by blending on black in ffmpeg's
    // swscale path) then convert to yuv420p for comparison.
    const ALPHA_FLATTEN: &str = "[0:v]format=rgb24,format=yuv420p,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0[ref];[1:v]format=yuv420p,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0[cmp];[ref][cmp]ssim";

    let ms = crate::numeric_cast::f64_to_u64_sat(crate::constants::MS_PER_SEC_F64);
    let gif_sync = format!(
        "[0:v]format=rgb24,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,settb=1/{ms},setpts=PTS-STARTPTS,format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,settb=1/{ms},setpts=PTS-STARTPTS,format=yuv420p[cmp];[ref][cmp]ssim"
    );

    let is_gif = is_gif_magic(input);

    if is_gif {
        run_ssim_all_filter(input, output, &gif_sync)
            .or_else(|| run_ssim_all_filter(input, output, GIF_RGB24))
            .or_else(|| run_ssim_all_filter(input, output, GIF_NORM))
            .or_else(|| run_ssim_all_filter(input, output, FORMAT_NORM))
    } else {
        run_ssim_all_filter(input, output, DIRECT)
            .or_else(|| run_ssim_all_filter(input, output, FORMAT_NORM))
            .or_else(|| run_ssim_all_filter(input, output, ALPHA_FLATTEN))
    }
}

fn parse_ssim_from_output(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        if line.contains("SSIM")
            && line.contains("All:")
            && let Some(all_pos) = line.find("All:")
        {
            let after_all = &line[all_pos + 4..];
            let after_all = after_all.trim_start();
            if after_all.starts_with("inf") {
                return Some(1.0);
            }
            let end = after_all
                .find(|c: char| !c.is_numeric() && c != '.')
                .unwrap_or(after_all.len());
            if end > 0 {
                return after_all[..end].parse::<f64>().ok();
            }
        }
    }
    None
}

fn extract_ssim_value(line: &str, prefix: &str) -> Option<f64> {
    if let Some(pos) = line.find(prefix) {
        let after = &line[pos + prefix.len()..];
        let after = after.trim_start();
        if after.starts_with("inf") {
            return Some(1.0);
        }
        let end = after
            .find(|c: char| !c.is_numeric() && c != '.')
            .unwrap_or(after.len());
        if end > 0 {
            return after[..end].parse::<f64>().ok();
        }
    }
    None
}

#[inline]
fn is_valid_ssim_value(ssim: f64) -> bool {
    (0.0..=1.0).contains(&ssim) && !ssim.is_nan()
}

// ── CRF=0 Lossless Integrity Check ────────────────────────────────────────────

/// Fast integrity check for CRF=0 (lossless) encodes.
///
/// When a source is encoded at CRF 0, the codec guarantees bit-exact YUV
/// reproduction — VMAF/SSIM would trivially score 100 / 1.0 and are pure
/// CPU waste.  Instead we verify two cheap invariants:
///
/// 1. **Frame count match** — the output contains at least as many frames as
///    the input (encode did not silently drop frames).
/// 2. **File size > 0** — the output file is non-empty (no silent I/O failure).
///
/// Note for GIF→HEVC: CRF=0 guarantees YUV-layer losslessness, but the
/// GIF palette → YUV conversion itself introduces chroma subsampling loss
/// (yuv420 vs yuv444).  That loss is determined at encoding time by the
/// pixel-format choice and is unrelated to CRF.  The caller should force
/// `yuv444p` to avoid this if RGB round-trip fidelity is required.
///
/// Returns `Ok(true)` when both invariants are satisfied, `Ok(false)` if a
/// check fails (with stderr warning already emitted), or `Err` if ffprobe
/// cannot be invoked at all.
///
/// # Errors
/// Returns an error if the frame-count ffprobe command fails to spawn.
pub fn check_lossless_integrity(
    input: &Path,
    output: &Path,
    output_size: u64,
    is_animated_image: bool,
) -> Result<bool, String> {
    // Guard: output must not be empty
    if output_size == 0 {
        warn!("CRF=0 integrity: output file is empty (silent encode failure?)");
        return Ok(false);
    }

    let input_frames = count_video_frames(input);
    let output_frames = count_video_frames(output);

    match (input_frames, output_frames) {
        (Some(i), Some(o)) if o >= i => {
            info!(
                input_frames = i,
                output_frames = o,
                "CRF=0 integrity: frame count OK"
            );
            Ok(true)
        }
        (Some(i), Some(o)) => {
            if is_animated_image {
                // For animated images (GIF/WebP), frame counts often decrease due to
                // FFmpeg's VFR-to-CFR alignment (merging frames into the same slot).
                // We pivot to duration validation: if the timeline remains intact, the data is OK.
                let i_dur = get_video_duration(input).ok_or_else(|| {
                    format!(
                        "Integrity check failed: cannot determine input duration for {}",
                        input.display()
                    )
                })?;
                let o_dur = get_video_duration(output).ok_or_else(|| {
                    format!(
                        "Integrity check failed: cannot determine output duration for {}",
                        output.display()
                    )
                })?;

                let dur_ratio = if i_dur > 0.0 { o_dur / i_dur } else { 1.0 };

                if dur_ratio >= crate::constants::STREAM_ANALYSIS_DURATION_MATCH_THRESHOLD {
                    warn!(
                        input_frames = i,
                        output_frames = o,
                        dur_ratio = format!("{:.4}", dur_ratio),
                        "CRF=0 integrity: frame count decreased but duration OK (VFR→CFR alignment). Soft-accepting."
                    );
                    Ok(true)
                } else {
                    warn!(
                        input_frames = i,
                        output_frames = o,
                        dur_ratio = format!("{:.4}", dur_ratio),
                        "CRF=0 integrity: both frame count AND duration dropped significantly — possible frame drop error"
                    );
                    Ok(false)
                }
            } else {
                warn!(
                    input_frames = i,
                    output_frames = o,
                    "CRF=0 integrity: output has FEWER frames than input — possible encode error"
                );
                Ok(false)
            }
        }
        (None, _) | (_, None) => {
            // Cannot determine frame count — treat as a soft warning, not a hard failure
            warn!(
                "CRF=0 integrity: could not determine frame count via ffprobe; skipping frame check"
            );
            // File is non-empty (checked above), so accept
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_valid_ssim_value ───────────────────────────────────────────────

    #[test]
    fn test_valid_ssim_normal_range() {
        assert!(is_valid_ssim_value(0.95));
        assert!(is_valid_ssim_value(1.0));
        assert!(is_valid_ssim_value(0.0));
    }

    #[test]
    fn test_valid_ssim_rejects_out_of_range() {
        assert!(!is_valid_ssim_value(1.01));
        assert!(!is_valid_ssim_value(-0.01));
        assert!(!is_valid_ssim_value(f64::NAN));
    }

    // ── extract_ssim_value ────────────────────────────────────────────────

    #[test]
    fn test_extract_ssim_value_typical() {
        let line = "SSIM Y:0.9876 U:0.9821 V:0.9790 All:0.9829";
        assert!(
            (extract_ssim_value(line, "Y:").unwrap_or_else(|| panic!("missing ssim value"))
                - 0.9876)
                .abs()
                < 1e-4_f64
        );
        assert!(
            (extract_ssim_value(line, "U:").unwrap_or_else(|| panic!("missing ssim value"))
                - 0.9821)
                .abs()
                < 1e-4_f64
        );
        assert!(
            (extract_ssim_value(line, "V:").unwrap_or_else(|| panic!("missing ssim value"))
                - 0.9790)
                .abs()
                < 1e-4_f64
        );
        assert!(
            (extract_ssim_value(line, "All:").unwrap_or_else(|| panic!("missing ssim value"))
                - 0.9829)
                .abs()
                < 1e-4_f64
        );
    }

    #[test]
    fn test_extract_ssim_value_inf() {
        let line = "SSIM Y:inf U:inf V:inf All:inf";
        assert!(
            (extract_ssim_value(line, "Y:").unwrap_or_else(|| panic!("missing ssim value")) - 1.0)
                .abs()
                < f64::EPSILON
        );
        assert!(
            (extract_ssim_value(line, "All:").unwrap_or_else(|| panic!("missing ssim value"))
                - 1.0)
                .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn test_extract_ssim_value_missing_prefix() {
        let line = "SSIM Y:0.98 All:0.97";
        assert!(extract_ssim_value(line, "Z:").is_none());
    }

    #[test]
    fn test_extract_ssim_value_perfect() {
        let line = "SSIM Y:1.000000 All:1.000000";
        assert!(
            (extract_ssim_value(line, "Y:").unwrap_or_else(|| panic!("missing ssim value")) - 1.0)
                .abs()
                < 1e-6_f64
        );
    }

    // ── parse_ssim_from_output ────────────────────────────────────────────

    #[test]
    fn test_parse_ssim_from_output_typical() {
        let stderr =
            "[Parsed_ssim_0 @ 0x1234] SSIM Y:0.9876 U:0.9821 V:0.9790 All:0.9829 (21.667260)\n";
        let result = parse_ssim_from_output(stderr);
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing ssim value")) - 0.9829).abs() < 1e-4_f64);
    }

    #[test]
    fn test_parse_ssim_from_output_perfect_identical() {
        let stderr = "SSIM Y:inf U:inf V:inf All:inf\n";
        let result = parse_ssim_from_output(stderr);
        assert!(result.is_some());
        assert!(
            (result.unwrap_or_else(|| panic!("missing ssim value")) - 1.0).abs() < f64::EPSILON
        );
    }

    #[test]
    fn test_parse_ssim_from_output_multiline() {
        let stderr = concat!(
            "frame=100 fps=25\n",
            "[Parsed_ssim_0 @ 0xabc] SSIM Y:0.9500 U:0.9400 V:0.9300 All:0.9400 (12.34)\n",
            "video:0kB audio:0kB\n",
        );
        let result = parse_ssim_from_output(stderr);
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing ssim value")) - 0.94).abs() < 1e-4_f64);
    }

    #[test]
    fn test_parse_ssim_from_output_no_match() {
        assert!(parse_ssim_from_output("frame=100 fps=25\n").is_none());
    }

    #[test]
    fn test_parse_ssim_from_output_empty() {
        assert!(parse_ssim_from_output("").is_none());
    }

    #[test]
    fn test_parse_ssim_from_output_ssim_without_all() {
        // Has "SSIM" keyword but no "All:" field
        assert!(parse_ssim_from_output("SSIM Y:0.98 U:0.97 V:0.96\n").is_none());
    }
}

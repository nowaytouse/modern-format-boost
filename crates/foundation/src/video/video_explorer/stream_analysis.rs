//! Stream Analysis Module
//!
//! This module is responsible for video stream analysis and quality assessment,
//! including:
//! - SSIM (Structural Similarity Index) calculation
//! - PSNR (Peak Signal-to-Noise Ratio) calculation
//! - MS-SSIM (Multi-Scale SSIM) calculation
//! - Video duration detection
//! - Quality threshold validation
//! - Lossless integrity checks (CRF=0 fast-path: frame count + file size only)

use crate::FfmpegBuilder;
use crate::builder_base::ToolBuilder;
use anyhow::{Context, Result};
use std::path::Path;

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

/// Probe video duration with ffprobe.
///
/// # Errors
/// Returns an error when ffprobe cannot be started, exits unsuccessfully, or
/// returns malformed/non-finite duration output.
pub fn get_video_duration(input: &Path) -> anyhow::Result<f64> {
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
            crate::media_conversion_gate::explore_precheck_degraded_audit(
                "explore_audit",
                format!(
                    "Subprocess failed to start for duration check ({}): verify ffprobe is in PATH",
                    input.display()
                ),
            );
            anyhow::anyhow!(
                "Failed to start ffprobe duration probe for {}: {e}",
                input.display()
            )
        })?;

    if !output.status.success() {
        crate::media_conversion_gate::explore_precheck_degraded_audit(
            "explore_audit",
            format!(
                "Failed to read video duration for {} (non-zero exit status); container may be \
                 malformed",
                input.display()
            ),
        );
        anyhow::bail!(
            "Failed to read video duration for {}: ffprobe exited with {}",
            input.display(),
            output.status
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    let duration = trimmed.parse::<f64>().map_err(|err| {
        crate::media_conversion_gate::explore_precheck_degraded_audit(
            "explore_audit",
            format!(
                "Failed to parse duration output for {} as float: {err}",
                input.display()
            ),
        );
        anyhow::anyhow!(
            "Failed to parse video duration for {} from {:?}: {err}",
            input.display(),
            trimmed
        )
    })?;
    if !duration.is_finite() || duration < 0.0 {
        crate::media_conversion_gate::explore_precheck_degraded_audit(
            "explore_audit",
            format!(
                "Invalid duration output for {}: {duration}",
                input.display()
            ),
        );
        anyhow::bail!("Invalid video duration for {}: {duration}", input.display());
    }
    Ok(duration)
}

fn count_video_frames(path: &Path) -> Option<u64> {
    match is_gif_magic(path) {
        Ok(true) => {
            if let Some(frames) = crate::media_conversion_gate::explore_gif_frame_count_optional(
                crate::image_formats::gif::get_frame_count(path),
                path,
            ) {
                return Some(crate::numeric_cast::usize_to_u64(frames));
            }
        }
        Ok(false) => {}
        Err(err) => {
            crate::media_conversion_gate::explore_precheck_degraded_audit(
                "explore_audit",
                format!(
                    "Failed to probe GIF magic for frame count ({}): {err}",
                    path.display()
                ),
            );
            return None;
        }
    }

    let is_webp = crate::image::format_detect::detect_true_format(path)
        .is_ok_and(|format| format == crate::image::format_detect::FormatKind::WebP);
    if is_webp {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                crate::media_conversion_gate::explore_precheck_degraded_audit(
                    "explore_audit",
                    format!(
                        "Failed to read WebP file for frame counting ({}): {e}",
                        path.display()
                    ),
                );
                return None;
            }
        };
        if let Some(frames) = crate::media_conversion_gate::explore_webp_frame_count_optional(
            crate::image_formats::webp::count_frames_from_bytes(&data),
            path,
        ) {
            return Some(u64::from(frames));
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
            Err(_e) => {
                crate::media_conversion_gate::explore_precheck_degraded_audit(
                    "explore_audit",
                    format!(
                        "Subprocess failed to start for frame count ({}): verify ffprobe is in \
                         PATH",
                        path.display()
                    ),
                );
                return None;
            }
        };

        if !out.status.success() {
            crate::media_conversion_gate::explore_precheck_degraded_audit(
                "explore_audit",
                format!(
                    "Failed to count frames for {} (non-zero exit status)",
                    path.display()
                ),
            );
            return None;
        }

        let stdout = String::from_utf8_lossy(&out.stdout);
        let trimmed = stdout.trim();
        match trimmed.parse::<u64>() {
            Ok(count) => Some(count),
            Err(e) => {
                crate::media_conversion_gate::explore_precheck_degraded_audit(
                    "explore_audit",
                    format!(
                        "Failed to parse frame count output for {} as integer: {e}",
                        path.display()
                    ),
                );
                None
            }
        }
    };

    let frames_count = try_ffprobe_count("-count_frames", "stream=nb_read_frames");
    match frames_count {
        Some(count) => Some(count),
        None => try_ffprobe_count("-count_packets", "stream=nb_read_packets"),
    }
}

#[must_use = "GIF magic probe errors must be propagated or explicitly audited"]
pub fn is_gif_magic(path: &Path) -> std::io::Result<bool> {
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut magic)?;
            Ok(())
        })
        .map(|()| &magic == b"GIF8")
        .map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!("GIF magic probe failed for {}: {err}", path.display()),
            )
        })
}

/// # Errors
/// Returns an error if required probes fail or metric output contains malformed
/// numeric tokens.
pub fn calculate_ssim_enhanced(input: &Path, output: &Path) -> Result<Option<f64>> {
    // GIF-specific path: force palette → yuv420p conversion on the reference side
    // before comparing with the yuv420p-encoded output.  This avoids all three
    // generic filters silently failing because ffmpeg cannot decode a GIF palette
    // stream into the same raw pixel layout as the HEVC output.
    let is_gif = match is_gif_magic(input) {
        Ok(value) => value,
        Err(err) => {
            crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                "ssim_gif_magic_probe_failed",
                format!("{}: {err}", input.display()),
            );
            return Err(err).context("GIF magic probe failed before enhanced SSIM");
        }
    };

    let mut filters: Vec<(String, String)> = Vec::new();
    if is_gif {
        let ms = crate::numeric_cast::f64_to_u64_strict(crate::constants::MS_PER_SEC_F64, "ms")
            .context("failed to build GIF SSIM timebase")?;
        filters.push((
            "gif_sync".to_string(),
            format!(
                "[0:v]format=rgb24,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,settb=1/{ms},\
                 setpts=PTS-STARTPTS,format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:\
                 0,settb=1/{ms},setpts=PTS-STARTPTS,format=yuv420p[cmp];[ref][cmp]ssim"
            ),
        ));
        filters.push((
            "gif_palette".to_string(),
            "[0:v]format=rgb24,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:\
             v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];[ref][cmp]ssim"
                .to_string(),
        ));
        filters.push((
            "gif_pad_even".to_string(),
            "[0:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:v]pad='iw+mod(iw,\
             2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];[ref][cmp]ssim"
                .to_string(),
        ));
    } else {
        filters.push((
            "standard".to_string(),
            "[0:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0[ref];[ref][1:v]ssim".to_string(),
        ));
        filters.push((
            "format_convert".to_string(),
            "[0:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:v]pad='iw+mod(iw,\
             2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];[ref][cmp]ssim"
                .to_string(),
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
                if let Some(ssim) = parse_ssim_from_output(&stderr)? {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_FFMPEG,
                        format!("SSIM calculated via {name}: {ssim:.4}")
                    );
                    return Ok(Some(ssim));
                }
            }
            Err(e) => {
                crate::media_conversion_gate::explore_precheck_degraded_audit(
                    "explore_audit",
                    format!("ffmpeg failed for {name}: {e}"),
                );
            }
        }
    }

    crate::media_conversion_gate::explore_precheck_degraded_audit(
        "explore_ssim_audit",
        "ALL SSIM CALCULATION METHODS FAILED",
    );
    Ok(None)
}

/// Run ffmpeg with the given lavfi filter and parse SSIM Y/U/V/All from stderr.
fn run_ssim_all_filter(
    input: &Path,
    output: &Path,
    lavfi: &str,
) -> Result<Option<(f64, f64, f64, f64)>> {
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
            crate::media_conversion_gate::explore_precheck_degraded_audit(
                "explore_ssim_audit",
                format!(
                    "Failed to start ffmpeg for SSIM-All calculation ({} -> {}): {e}",
                    input.display(),
                    output.display()
                ),
            );
            return Err(e).context("failed to start ffmpeg for SSIM-All calculation");
        }
    };

    // Some filters (ssim) might return non-zero exit code if the streams
    // end at slightly different points for GIFs, even if the result is valid.
    // We parse stderr regardless of out.status.
    let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);
    for line in stderr.lines() {
        if line.contains("SSIM Y:") && line.contains("All:") {
            let y = extract_ssim_value(line, "Y:")?;
            let u = extract_ssim_value(line, "U:")?;
            let v = extract_ssim_value(line, "V:")?;
            let all = extract_ssim_value(line, "All:")?;
            if let (Some(y), Some(u), Some(v), Some(all)) = (y, u, v, all) {
                if let Some(sealed) =
                    crate::video_explorer::precision::seal_ssim_yuv_all_bundle(y, u, v, all)
                {
                    return Ok(Some(sealed));
                }
                crate::media_conversion_gate::explore_metric_parse_reject_audit(
                    "ssim_all",
                    format!("Y/U/V/All rejected (Y={y:.6}, U={u:.6}, V={v:.6}, All={all:.6})"),
                );
            }
        }
    }
    Ok(None)
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
/// 3. Alpha composite on black (transparent WebP/PNG vs HEVC which has no
///    alpha): converts the input to rgb24 (discarding alpha channel) then
///    yuv420p, matching the actual encoder behaviour.
/// # Errors
/// Returns an error if required probes fail or metric output contains malformed
/// numeric tokens.
pub fn calculate_ssim_all(input: &Path, output: &Path) -> Result<Option<(f64, f64, f64, f64)>> {
    // GIF-specific chains: render palette → rgb24 → yuv420p before comparing.
    // Use padding (upward to even) to match encoder's padding logic, and use
    // settb/setpts to sync pts.
    const GIF_RGB24: &str = "[0:v]format=rgb24,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,\
                             format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,\
                             format=yuv420p[cmp];[ref][cmp]ssim";
    const GIF_NORM: &str = "[0:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[ref];[1:\
                            v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,format=yuv420p[cmp];\
                            [ref][cmp]ssim";

    // Generic chains (pad to even)
    const DIRECT: &str = "[0:v][1:v]ssim";
    const FORMAT_NORM: &str = "[0:v]format=yuv420p,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0[ref];[1:\
                               v]format=yuv420p,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0[cmp];\
                               [ref][cmp]ssim";
    // Alpha-composite on black without the deprecated premultiply=inplace=1 filter:
    // decode to rgb24 (which discards alpha by blending on black in ffmpeg's
    // swscale path) then convert to yuv420p for comparison.
    const ALPHA_FLATTEN: &str = "[0:v]format=rgb24,format=yuv420p,pad='iw+mod(iw,2)':'ih+mod(ih,\
                                 2)':0:0[ref];[1:v]format=yuv420p,pad='iw+mod(iw,2)':'ih+mod(ih,\
                                 2)':0:0[cmp];[ref][cmp]ssim";

    let ms = crate::numeric_cast::f64_to_u64_strict(crate::constants::MS_PER_SEC_F64, "MS_PER_SEC")
        .context("failed to build SSIM-All GIF timebase")?;
    let gif_sync = format!(
        "[0:v]format=rgb24,pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,settb=1/{ms},setpts=PTS-STARTPTS,\
         format=yuv420p[ref];[1:v]pad='iw+mod(iw,2)':'ih+mod(ih,2)':0:0,settb=1/{ms},\
         setpts=PTS-STARTPTS,format=yuv420p[cmp];[ref][cmp]ssim"
    );

    let is_gif = match is_gif_magic(input) {
        Ok(value) => value,
        Err(err) => {
            crate::media_conversion_gate::explore_ssim_metric_degraded_audit(
                "ssim_all_gif_magic_probe_failed",
                format!("{}: {err}", input.display()),
            );
            return Err(err).context("GIF magic probe failed before SSIM-All");
        }
    };

    if is_gif {
        let mut ssim = run_ssim_all_filter(input, output, &gif_sync)?;
        if ssim.is_none() {
            ssim = run_ssim_all_filter(input, output, GIF_RGB24)?;
        }
        if ssim.is_none() {
            ssim = run_ssim_all_filter(input, output, GIF_NORM)?;
        }
        if ssim.is_none() {
            ssim = run_ssim_all_filter(input, output, FORMAT_NORM)?;
        }
        Ok(ssim)
    } else {
        let mut ssim = run_ssim_all_filter(input, output, DIRECT)?;
        if ssim.is_none() {
            ssim = run_ssim_all_filter(input, output, FORMAT_NORM)?;
        }
        if ssim.is_none() {
            ssim = run_ssim_all_filter(input, output, ALPHA_FLATTEN)?;
        }
        Ok(ssim)
    }
}

fn parse_ssim_from_output(stderr: &str) -> Result<Option<f64>> {
    for line in stderr.lines() {
        if line.contains("SSIM")
            && line.contains("All:")
            && let Some(all_pos) = line.find("All:")
        {
            let after_all = &line[all_pos + 4..];
            if let Some(sealed) =
                crate::video_explorer::precision::parse_explore_ssim_metric_token(after_all)
                    .map_err(|err| anyhow::anyhow!("failed to parse SSIM metric token: {err}"))?
            {
                return Ok(Some(sealed));
            }
        }
    }
    Ok(None)
}

fn extract_ssim_value(line: &str, prefix: &str) -> Result<Option<f64>> {
    let Some(pos) = line.find(prefix) else {
        return Ok(None);
    };
    let after = &line[pos + prefix.len()..];
    crate::video_explorer::precision::parse_explore_ssim_metric_token(after).map_err(|err| {
        anyhow::anyhow!("failed to parse SSIM metric token for prefix {prefix}: {err}")
    })
}

// ── CRF=0 Lossless Integrity Check
// ────────────────────────────────────────────

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
        crate::media_conversion_gate::explore_precheck_degraded_audit(
            "explore_audit",
            "CRF=0 integrity: output file is empty (silent encode failure?)",
        );
        return Ok(false);
    }

    let input_frames = count_video_frames(input);
    let output_frames = count_video_frames(output);

    match (input_frames, output_frames) {
        (Some(i), Some(o)) if o >= i => {
            crate::log_detail!(format!(
                "CRF=0 integrity: frame count OK (input={i}, output={o})"
            ));
            Ok(true)
        }
        (Some(i), Some(o)) => {
            if is_animated_image {
                // For animated images (GIF/WebP), frame counts often decrease due to
                // FFmpeg's VFR-to-CFR alignment (merging frames into the same slot).
                // We pivot to duration validation: if the timeline remains intact, the data is
                // OK.
                let i_dur = get_video_duration(input).map_err(|err| {
                    format!(
                        "Integrity check failed: cannot determine input duration for {}: {err}",
                        input.display(),
                    )
                })?;
                let o_dur = get_video_duration(output).map_err(|err| {
                    format!(
                        "Integrity check failed: cannot determine output duration for {}: {err}",
                        output.display(),
                    )
                })?;

                let dur_ratio = if i_dur > 0.0 { o_dur / i_dur } else { 1.0 };

                if dur_ratio >= crate::constants::STREAM_ANALYSIS_DURATION_MATCH_THRESHOLD {
                    Ok(true)
                } else {
                    crate::media_conversion_gate::explore_precheck_degraded_audit(
                        "explore_audit",
                        format!(
                            "CRF=0 integrity: both frame count AND duration dropped significantly \
                             — possible frame drop error (input={i}, output={o}, \
                             ratio={dur_ratio:.4})"
                        ),
                    );
                    Ok(false)
                }
            } else {
                crate::media_conversion_gate::explore_precheck_degraded_audit(
                    "explore_audit",
                    format!(
                        "CRF=0 integrity: output has FEWER frames than input — possible encode \
                         error (input={i}, output={o})"
                    ),
                );
                Ok(false)
            }
        }
        (None, _) | (_, None) => {
            // Cannot determine frame count — soft warning only; file is non-empty so
            // accept.
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_ssim(result: Result<Option<f64>>) -> Option<f64> {
        result.unwrap_or_else(|err| panic!("SSIM parse failed: {err}"))
    }

    fn ssim_value(result: Result<Option<f64>>) -> f64 {
        ok_ssim(result).unwrap_or_else(|| panic!("missing ssim value"))
    }

    // ── is_valid_ssim (precision contract) ─────────────────────────────────

    #[test]
    fn test_valid_ssim_normal_range() {
        assert!(crate::video_explorer::precision::is_valid_ssim(0.95));
        assert!(crate::video_explorer::precision::is_valid_ssim(1.0));
        assert!(crate::video_explorer::precision::is_valid_ssim(0.0));
    }

    #[test]
    fn test_valid_ssim_rejects_out_of_range() {
        assert!(!crate::video_explorer::precision::is_valid_ssim(1.01));
        assert!(!crate::video_explorer::precision::is_valid_ssim(-0.01));
        assert!(!crate::video_explorer::precision::is_valid_ssim(f64::NAN));
    }

    // ── extract_ssim_value ────────────────────────────────────────────────

    #[test]
    fn test_extract_ssim_value_typical() {
        let line = "SSIM Y:0.9876 U:0.9821 V:0.9790 All:0.9829";
        assert!((ssim_value(extract_ssim_value(line, "Y:")) - 0.9876).abs() < 1e-4_f64);
        assert!((ssim_value(extract_ssim_value(line, "U:")) - 0.9821).abs() < 1e-4_f64);
        assert!((ssim_value(extract_ssim_value(line, "V:")) - 0.9790).abs() < 1e-4_f64);
        assert!((ssim_value(extract_ssim_value(line, "All:")) - 0.9829).abs() < 1e-4_f64);
    }

    #[test]
    fn test_extract_ssim_value_inf() {
        let line = "SSIM Y:inf U:inf V:inf All:inf";
        assert!((ssim_value(extract_ssim_value(line, "Y:")) - 1.0).abs() < f64::EPSILON);
        assert!((ssim_value(extract_ssim_value(line, "All:")) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_extract_ssim_value_missing_prefix() {
        let line = "SSIM Y:0.98 All:0.97";
        assert!(ok_ssim(extract_ssim_value(line, "Z:")).is_none());
    }

    #[test]
    fn get_video_duration_missing_file_returns_error_not_none() {
        let missing = Path::new("/tmp/mfb_missing_duration_probe_input.mov");
        let err = get_video_duration(missing)
            .expect_err("missing duration probe target must fail closed");
        assert!(
            err.to_string().contains("duration"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn is_gif_magic_missing_file_returns_error_not_false() {
        let missing = Path::new("/tmp/mfb_missing_gif_magic_probe.gif");
        let err =
            is_gif_magic(missing).expect_err("missing GIF magic probe target must fail closed");
        assert!(err.to_string().contains("GIF"), "unexpected error: {err}");
    }

    #[test]
    fn test_extract_ssim_value_perfect() {
        let line = "SSIM Y:1.000000 All:1.000000";
        assert!((ssim_value(extract_ssim_value(line, "Y:")) - 1.0).abs() < 1e-6_f64);
    }

    // ── parse_ssim_from_output ────────────────────────────────────────────

    #[test]
    fn test_parse_ssim_from_output_typical() {
        let stderr =
            "[Parsed_ssim_0 @ 0x1234] SSIM Y:0.9876 U:0.9821 V:0.9790 All:0.9829 (21.667260)\n";
        let result = parse_ssim_from_output(stderr);
        let result = ok_ssim(result);
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing ssim value")) - 0.9829).abs() < 1e-4_f64);
    }

    #[test]
    fn test_parse_ssim_from_output_perfect_identical() {
        let stderr = "SSIM Y:inf U:inf V:inf All:inf\n";
        let result = parse_ssim_from_output(stderr);
        let result = ok_ssim(result);
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
        let result = ok_ssim(result);
        assert!(result.is_some());
        assert!((result.unwrap_or_else(|| panic!("missing ssim value")) - 0.94).abs() < 1e-4_f64);
    }

    #[test]
    fn test_parse_ssim_from_output_no_match() {
        assert!(ok_ssim(parse_ssim_from_output("frame=100 fps=25\n")).is_none());
    }

    #[test]
    fn test_parse_ssim_from_output_empty() {
        assert!(ok_ssim(parse_ssim_from_output("")).is_none());
    }

    #[test]
    fn test_parse_ssim_from_output_ssim_without_all() {
        // Has "SSIM" keyword but no "All:" field
        assert!(ok_ssim(parse_ssim_from_output("SSIM Y:0.98 U:0.97 V:0.96\n")).is_none());
    }
}

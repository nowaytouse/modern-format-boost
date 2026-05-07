//! x265 Direct CPU Encoder Module
//!
//! 🔥 v6.9.17: CPU Encoding Architecture - Direct encoding using x265 command-line tool
//!
//! ## Architectural Design
//!
//! Due to the lack of libx265 support in the system `FFmpeg`, a three-step encoding process is used:
//! 1. `FFmpeg Decoding` → Y4M (raw YUV)
//! 2. x265 Encoding → HEVC bitstream
//! 3. `FFmpeg Muxing` → MP4 Container
//!
//! ## Advantages
//! - `Independent of FFmpeg compilation options`
//! - Full CRF control (sub-integer precision)
//! - Higher SSIM quality (≥0.98 vs `VideoToolbox` ~0.95)
//! - Strict CPU encoding path (no GPU fallback)

use anyhow::{Context, Result, bail};
use std::fmt::Write as FmtWrite;
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, error, warn};

#[derive(Debug, Clone)]
pub struct X265Config {
    pub crf: f32,
    pub preset: String,
    pub threads: usize,
    pub container: String,
    pub sample_duration: Option<f32>,
    pub preserve_audio: bool,
    /// Pixel format to use for the YUV pipe. Set to "yuv420p10le" for 10-bit HDR content.
    pub pix_fmt: String,
    /// HDR colour primaries (e.g. "bt2020")
    pub color_primaries: Option<String>,
    /// HDR transfer characteristics (e.g. "smpte2084", "arib-std-b67")
    pub color_trc: Option<String>,
    /// HDR matrix coefficients (e.g. "bt2020nc")
    pub colorspace: Option<String>,
    /// HDR10 mastering display metadata in ffmpeg format
    pub mastering_display: Option<String>,
    /// HDR10 content light level: "MaxCLL,MaxFALL"
    pub max_cll: Option<String>,
    /// Audio codec of the source (used to decide copy vs transcode in mux step)
    pub audio_codec: Option<String>,
    /// Whether the source has subtitle streams
    pub has_subtitles: bool,
    /// Codec name of the first subtitle stream
    pub subtitle_codec: Option<String>,
    /// Whether to apply Apple-specific compatibility fixes (e.g. hvc1 tag)
    pub apple_compat: bool,
    /// Additional raw x265 parameters (e.g., "aq-mode=3:aq-strength=1.0")
    pub x265_params: Option<String>,
}

impl Default for X265Config {
    fn default() -> Self {
        Self {
            crf: 23.0,
            preset: crate::types::EncoderPreset::Medium.hevc_name().to_string(),
            threads: crate::thread_manager::get_optimal_threads(),
            container: "mp4".to_string(),
            sample_duration: None,
            preserve_audio: true,
            pix_fmt: "yuv420p".to_string(),
            color_primaries: None,
            color_trc: None,
            colorspace: None,
            mastering_display: None,
            max_cll: None,
            audio_codec: None,
            has_subtitles: false,
            subtitle_codec: None,
            apple_compat: false,
            x265_params: None,
        }
    }
}

/// Encode an image to HEVC using x265.
///
/// # Errors
/// Returns an error if encoding fails.
pub fn encode_with_x265(
    input: &Path,
    output: &Path,
    config: &X265Config,
    vf_args: &[String],
) -> Result<u64> {
    let hevc_temp = tempfile::Builder::new()
        .suffix(".hevc")
        .tempfile()
        .context("Failed to create temporary HEVC file")?;
    let hevc_file = hevc_temp.path().to_path_buf();

    let encode_result = encode_to_hevc(input, &hevc_file, config, vf_args)?;

    if !encode_result {
        error!("x265 encoding failed❌");
        bail!("x265 encoding failed");
    }

    mux_hevc_to_container(input, &hevc_file, output, config)?;

    drop(hevc_temp);

    let output_size = std::fs::metadata(output)
        .context("Failed to get output file size")?
        .len();

    debug!(
        output_size = output_size,
        output_path = ?output,
        "✅ x265 CPU encoding complete"
    );

    Ok(output_size)
}

/// Encode a .y4m file directly with x265 (no `FFmpeg` pipe). Avoids Broken pipe when
/// the pipeline `FFmpeg` stdout → x265 stdin is used with low-fps or odd y4m streams.
fn encode_y4m_direct(
    input: &Path,
    hevc_output: &Path,
    config: &X265Config,
    start_time: std::time::Instant,
) -> Result<bool> {
    debug!(
        "Starting x265 encoding with CRF {:.1}, preset {}",
        config.crf, config.preset
    );

    let output = crate::tool_builders::X265Builder::new()
        .y4m(true)
        .input(input)
        .output(hevc_output)
        .crf(config.crf)
        .lossless(config.crf == 0.0)
        .preset(&config.preset)
        .pools(config.threads.to_string())
        .log_level("error")
        .build()
        .output()
        .context("Failed to run x265")?;

    let duration = start_time.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        error!(
            exit_code = ?output.status.code(),
            duration_secs = duration.as_secs_f64(),
            stderr = %stderr,
            "x265 direct encode failed"
        );
        if !stderr.is_empty() {
            eprintln!("x265 stderr:\n{stderr}");
        }
        bail!(
            "x265 encode failed with exit code {:?}",
            output.status.code()
        );
    }

    debug!(
        duration_secs = duration.as_secs_f64(),
        output_file = ?hevc_output,
        "x265 encoding completed successfully (direct .y4m)"
    );
    Ok(true)
}

fn spawn_log_thread(
    stderr: std::process::ChildStderr,
    label: &'static str,
    filters: Option<&'static [&'static str]>,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Read as _Read};
        const MAX_TOTAL_LOG_SIZE: usize = 1_048_576; // 1MB
        const MAX_LINES: usize = 100_000;
        const STREAM_LIMIT: u64 = 10 * 1024 * 1024; // 10MB limit to prevent run-away logging

        let reader = BufReader::with_capacity(8192, stderr.take(STREAM_LIMIT));
        let mut output = String::with_capacity(64 * 1024);

        for line in reader.lines().take(MAX_LINES) {
            match line {
                Ok(line) => {
                    if crate::progress_mode::is_verbose_mode() {
                        let should_emit =
                            filters.is_none_or(|f| f.iter().any(|&s| line.contains(s)));
                        if should_emit {
                            crate::progress_mode::emit_stderr(&format!("   [{label}] {line}"));
                        }
                    }

                    if output.len() + line.len() + 1 > MAX_TOTAL_LOG_SIZE {
                        break;
                    }
                    output.push_str(&line);
                    output.push('\n');
                }
                Err(err) => {
                    warn!("Failed to read {label} stderr: {err}");
                    let _ = writeln!(output, "[stderr read error: {err}]");
                    break;
                }
            }
        }
        output
    })
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn encode_to_hevc(
    input: &Path,
    hevc_output: &Path,
    config: &X265Config,
    vf_args: &[String],
) -> Result<bool> {
    let start_time = std::time::Instant::now();

    // When input is already .y4m (e.g. from dynamic_mapping temp), run x265 directly
    // to avoid FFmpeg→pipe→x265 which can cause Broken pipe (x265 closing stdin early).
    let is_y4m = input
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("y4m"));
    if is_y4m {
        return encode_y4m_direct(input, hevc_output, config, start_time);
    }

    let mut ffmpeg_cmd = build_ffmpeg_y4m_decode_command(input, config, vf_args);
    ffmpeg_cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let log_level = if crate::progress_mode::is_verbose_mode() {
        "info"
    } else {
        "error"
    };

    let mut x265_builder = crate::tool_builders::X265Builder::new();
    x265_builder
        .y4m(true)
        .input("-")
        .output(hevc_output)
        .crf(config.crf)
        .preset(&config.preset)
        .pools(config.threads.to_string())
        .log_level(log_level)
        .arg("--repeat-headers");

    if config.crf == 0.0 {
        x265_builder.lossless(true);
    }

    if let Some(params) = &config.x265_params {
        for p in params.split(':') {
            x265_builder.arg(format!("--{p}"));
        }
    }

    // HDR-specific x265 options: enabled when the source is 10-bit or has explicit HDR metadata.
    // Also covers BT.2020 primaries without a matching PQ/HLG transfer (some pipelines drop
    // the transfer tag even though the source is wide-gamut HDR).
    let is_hdr_content = config.pix_fmt.contains("10")
        || config.mastering_display.is_some()
        || config.max_cll.is_some()
        || matches!(
            config.color_trc.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        )
        || matches!(config.color_primaries.as_deref(), Some("bt2020"));
    if is_hdr_content {
        x265_builder.hdr10_opt(true).repeat_headers(true);

        if let Some(ref cp) = config.color_primaries {
            x265_builder.colorprim(cp);
        }
        if let Some(ref trc) = config.color_trc {
            x265_builder.transfer(trc);
        }
        if let Some(ref cs) = config.colorspace {
            x265_builder.colormatrix(cs);
        }
        if let Some(ref md) = config.mastering_display {
            x265_builder.master_display(md);
        }
        if let Some(ref cll) = config.max_cll {
            x265_builder.max_cll(cll);
        }
    }

    let mut x265_cmd = x265_builder.build();

    x265_cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut ffmpeg_child = ffmpeg_cmd
        .spawn()
        .context("Failed to spawn ffmpeg decode process")?;

    let mut x265_child = x265_cmd
        .spawn()
        .context("Failed to spawn x265 encode process")?;

    let ffmpeg_stderr_thread = ffmpeg_child
        .stderr
        .take()
        .map(|stderr| spawn_log_thread(stderr, "ffmpeg-decode", None));

    let x265_stderr_thread = x265_child
        .stderr
        .take()
        .map(|stderr| spawn_log_thread(stderr, "x265-encode", Some(&["[info]", "frame"])));

    if let (Some(mut ffmpeg_out), Some(mut x265_in)) =
        (ffmpeg_child.stdout.take(), x265_child.stdin.take())
    {
        let transfer_thread =
            std::thread::spawn(move || std::io::copy(&mut ffmpeg_out, &mut x265_in));

        // Join pipe-copy thread first so we see BrokenPipe before process exit codes.
        let copy_result: Result<Result<u64, std::io::Error>, _> = transfer_thread.join();
        let pipe_io_error = copy_result.as_ref().ok().and_then(|r| r.as_ref().err());
        let is_broken_pipe = pipe_io_error.is_some_and(|e| {
            use std::io::ErrorKind;
            matches!(e.kind(), ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
        });

        let x265_status = x265_child.wait().context("Failed to wait for x265")?;
        let ffmpeg_status = ffmpeg_child.wait().context("Failed to wait for ffmpeg")?;

        let duration = start_time.elapsed();

        let ffmpeg_stderr = ffmpeg_stderr_thread
            .and_then(|h: std::thread::JoinHandle<String>| h.join().ok())
            .unwrap_or_default();
        let x265_stderr = x265_stderr_thread
            .and_then(|h: std::thread::JoinHandle<String>| h.join().ok())
            .unwrap_or_default();

        if !ffmpeg_status.success() {
            error!(
                exit_code = ?ffmpeg_status.code(),
                duration_secs = duration.as_secs_f64(),
                stderr = %ffmpeg_stderr,
                pipe_broken = is_broken_pipe,
                "FFmpeg decode failed"
            );
            if is_broken_pipe {
                warn!(
                    "Pipe broken: reader (x265) likely closed stdin first; x265 may have exited or rejected the stream"
                );
                if !x265_stderr.is_empty() {
                    eprintln!("x265 stderr (often shows why pipe closed):\n{x265_stderr}");
                }
            }
            if !ffmpeg_stderr.is_empty() {
                eprintln!("FFmpeg error output:\n{ffmpeg_stderr}");
            }
            bail!(
                "FFmpeg decode failed (exit_code: {:?})\n\nStderr:\n{}",
                ffmpeg_status.code(),
                ffmpeg_stderr.trim()
            );
        }

        if !x265_status.success() {
            error!(
                exit_code = ?x265_status.code(),
                duration_secs = duration.as_secs_f64(),
                stderr = %x265_stderr,
                pipe_broken = is_broken_pipe,
                "x265 encode failed"
            );
            if is_broken_pipe {
                warn!("Pipe broken: encoder (x265) likely exited first; check x265 stderr above");
            }
            if !x265_stderr.is_empty() {
                eprintln!("x265 error output:\n{x265_stderr}");
            }
            bail!("x265 encode failed with exit code {:?}", x265_status.code());
        }

        if let Ok(Err(io_err)) = &copy_result {
            error!(
                io_error = %io_err,
                kind = ?io_err.kind(),
                "Pipe copy failed (decoder and encoder both reported success)"
            );
            if is_broken_pipe {
                bail!("Pipe broken during copy (ffmpeg→x265): {io_err}");
            }
            bail!("Pipe I/O error: {io_err}");
        }
        if let Err(join_err) = copy_result {
            error!("Pipe copy thread panicked: {:?}", join_err);
            bail!("Pipe copy thread panicked: {join_err:?}");
        }

        debug!(
            duration_secs = duration.as_secs_f64(),
            output_file = ?hevc_output,
            "x265 encoding completed successfully"
        );

        Ok(true)
    } else {
        error!("Failed to connect ffmpeg and x265 pipes");
        bail!("Failed to connect ffmpeg and x265 pipes");
    }
}

fn build_ffmpeg_y4m_decode_command(
    input: &Path,
    config: &X265Config,
    vf_args: &[String],
) -> Command {
    let mut ffmpeg_builder = crate::ffmpeg_builder::FfmpegBuilder::new();
    ffmpeg_builder
        .overwrite()
        .input(input)
        .arg("-map")
        .arg("0:v:0")
        .arg("-an")
        .format("yuv4mpegpipe")
        .pix_fmt_str(&config.pix_fmt);

    if y4m_pipe_requires_relaxed_strictness(&config.pix_fmt) {
        // FFmpeg 8.1 rejects extended Y4M pixel formats such as yuv420p10le unless
        // strictness is lowered. x265 accepts these headers, so enable them here.
        ffmpeg_builder.arg("-strict").arg("-1");
    }

    if let Some(sample_duration) = config.sample_duration {
        ffmpeg_builder.arg("-t").arg(sample_duration.to_string());
    }

    for arg in vf_args {
        ffmpeg_builder.arg(arg);
    }

    ffmpeg_builder.output_pipe().build()
}

fn y4m_pipe_requires_relaxed_strictness(pix_fmt: &str) -> bool {
    !matches!(pix_fmt, "yuv420p" | "yuv422p" | "yuv444p" | "gray")
}

fn is_image_container(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "avif" | "heic" | "heif" | "gif" | "webp" | "png" | "jpg" | "jpeg" | "bmp" | "tiff"
    )
}

fn mux_hevc_to_container(
    original_input: &Path,
    hevc_file: &Path,
    output: &Path,
    config: &X265Config,
) -> Result<()> {
    let start_time = std::time::Instant::now();

    // Image containers (AVIF, HEIC, GIF, WebP, …) cannot carry audio streams.
    // Attempting to demux audio from them causes "Not yet implemented in FFmpeg".
    let input_is_image = is_image_container(original_input);
    let mut mux_builder = crate::ffmpeg_builder::FfmpegBuilder::new();
    mux_builder.overwrite().input(hevc_file);

    if config.preserve_audio && !input_is_image {
        mux_builder.input(original_input);
        // Map: video from HEVC bitstream (input 0), all audio + subtitle from original (input 1)
        mux_builder.arg("-map").arg("0:v:0");
        mux_builder.arg("-map").arg("1:a?");
        mux_builder.codec_video("copy");

        // Audio: copy when compatible, transcode only for incompatible codecs
        let audio_args =
            crate::audio_args_for_container(config.audio_codec.as_deref(), &config.container);
        for arg in &audio_args {
            // Skip -an since we already have -map 1:a?
            if arg != "-an" {
                mux_builder.arg(arg);
            }
        }

        // Subtitles: map and copy/transcode as appropriate for container
        if config.has_subtitles {
            mux_builder.arg("-map").arg("1:s?");
            let sub_args = crate::subtitle_args_for_container(
                true,
                config.subtitle_codec.as_deref(),
                &config.container,
            );
            for arg in sub_args {
                mux_builder.arg(arg);
            }
        }
    } else {
        // No audio: either disabled or source is an image format with no audio streams.
        mux_builder.codec_video("copy").arg("-an");
    }

    if (config.container == "mp4" || config.container == "mov") && config.apple_compat {
        mux_builder
            .tag_video("hvc1")
            .arg("-movflags")
            .arg("+faststart");
    } else if config.container == "mp4" || config.container == "mov" {
        mux_builder.arg("-movflags").arg("+faststart");
    }

    let mut cmd = mux_builder.output(output).build();
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());

    debug!("Starting FFmpeg muxing to {} container", config.container);

    let output_result = cmd.output().context("Failed to execute ffmpeg mux")?;

    let duration = start_time.elapsed();

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        error!(
            exit_code = ?output_result.status.code(),
            duration_secs = duration.as_secs_f64(),
            stderr = %stderr,
            "FFmpeg mux failed"
        );
        bail!("FFmpeg mux failed: {stderr}");
    }

    debug!(
        duration_secs = duration.as_secs_f64(),
        output_file = ?output,
        "FFmpeg mux completed successfully"
    );

    Ok(())
}

pub fn is_x265_available() -> bool {
    let result = crate::tool_builders::X265Builder::check_available();

    if result {
        debug!("x265 tool is available");
    } else {
        warn!("x265 tool is not available - install with: brew install x265");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_x265_available() {
        if is_x265_available() {
            println!("✅ x265 is available");
        } else {
            println!("⚠️  x265 not found - install with: brew install x265");
        }
    }

    #[test]
    fn ffmpeg_y4m_decode_relaxes_strictness_for_10bit_pipe_formats() {
        let config = X265Config {
            pix_fmt: "yuv420p10le".to_string(),
            ..Default::default()
        };
        let args: Vec<String> =
            build_ffmpeg_y4m_decode_command(Path::new("input.mov"), &config, &[])
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();

        assert!(
            args.windows(2)
                .any(|pair| pair == ["-pix_fmt", "yuv420p10le"])
        );
        assert!(args.windows(2).any(|pair| pair == ["-strict", "-1"]));
    }

    #[test]
    fn ffmpeg_y4m_decode_keeps_standard_pipe_formats_strict() {
        let config = X265Config::default();
        let args: Vec<String> =
            build_ffmpeg_y4m_decode_command(Path::new("input.mov"), &config, &[])
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();

        assert!(args.windows(2).any(|pair| pair == ["-pix_fmt", "yuv420p"]));
        assert!(!args.windows(2).any(|pair| pair == ["-strict", "-1"]));
    }
}

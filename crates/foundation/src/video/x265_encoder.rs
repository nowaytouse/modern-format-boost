//! x265 Direct CPU Encoder Module
//! CPU Encoding Architecture - Direct encoding using x265 command-line tool
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

use crate::builder_base::ToolBuilder;
use anyhow::{Context, Result, bail};
use std::fmt::Write as FmtWrite;
use std::path::Path;
use std::process::{Command, Stdio};

const TOKEN_DEBUG: &str = "{:?}";

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

impl X265Config {
    fn should_enable_hdr10_opt(&self) -> bool {
        crate::hdr::should_enable_x265_hdr10_opt(
            self.colorspace.as_deref(),
            self.color_trc.as_deref(),
            self.color_primaries.as_deref(),
            self.mastering_display.as_deref(),
            self.max_cll.as_deref(),
            crate::x265_params::has_hdr10plus_metadata(self.x265_params.as_deref()),
            &self.pix_fmt,
        )
    }

    fn should_emit_hdr10_metadata(&self) -> bool {
        crate::hdr::should_emit_x265_hdr10_metadata(
            self.colorspace.as_deref(),
            self.color_trc.as_deref(),
            self.color_primaries.as_deref(),
            self.mastering_display.as_deref(),
            self.max_cll.as_deref(),
            crate::x265_params::has_hdr10plus_metadata(self.x265_params.as_deref()),
        )
    }

    fn apply_x265_color_signaling(&self, x265_builder: &mut crate::tool_builders::X265Builder) {
        if self.should_emit_hdr10_metadata() {
            x265_builder.hdr10(true);
        }
        if self.should_enable_hdr10_opt() {
            x265_builder.hdr10_opt(true);
        }

        if let Some(cp) = self
            .color_primaries
            .as_deref()
            .filter(|value| !value.is_empty() && *value != "unknown")
        {
            x265_builder.colorprim(cp);
        }
        if let Some(trc) = self
            .color_trc
            .as_deref()
            .filter(|value| !value.is_empty() && *value != "unknown")
        {
            x265_builder.transfer(trc);
        }
        if let Some(cs) = self
            .colorspace
            .as_deref()
            .filter(|value| !value.is_empty() && *value != "unknown")
        {
            x265_builder.colormatrix(cs);
        }
        if self.should_emit_hdr10_metadata() {
            if let Some(md) = self
                .mastering_display
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                x265_builder.master_display(md);
            }
            if let Some(cll) = self
                .max_cll
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                x265_builder.max_cll(cll);
            }
        }
    }

    fn configure_x265_builder(
        &self,
        x265_builder: &mut crate::tool_builders::X265Builder,
        log_level: &str,
    ) {
        x265_builder
            .crf(self.crf)
            .preset(&self.preset)
            .pools(self.threads.to_string())
            .log_level(log_level)
            .repeat_headers(true);

        if crate::float_compare::approx_eq_crf(self.crf, 0.0) {
            x265_builder.lossless(true);
        }

        if let Some(params) = &self.x265_params {
            for p in params.split(':') {
                x265_builder.arg(format!("--{p}"));
            }
        }

        self.apply_x265_color_signaling(x265_builder);
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
    let hevc_temp = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "x265_hevc_bitstream",
        None,
        Some(".hevc"),
    )
    .context("Failed to create temporary HEVC file")?;
    let hevc_file = hevc_temp.path().to_path_buf();

    let encode_result = encode_to_hevc(input, &hevc_file, config, vf_args)?;

    if !encode_result {
        crate::log_error!(
            crate::infra::static_logs::messages::LABEL_ENCODER,
            crate::infra::static_logs::messages::MSG_ENCODE_BITSTREAM_FAIL
        );
        bail!("x265 encoding failed");
    }

    mux_hevc_to_container(input, &hevc_file, output, config)?;

    drop(hevc_temp);

    let output_size = std::fs::metadata(output)
        .context("Failed to get output file size")?
        .len();

    crate::log_detail!(
        &crate::infra::static_logs::messages::MSG_ENCODE_FINALIZED
            .replace("{}", &crate::format_bytes(output_size))
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
    crate::log_debug!(
        crate::infra::static_logs::messages::LABEL_ENCODER,
        format!(
            "Encoder Audit: Initiating direct encoding (crf={crf:.1}, preset={preset}, input={input_path})",
            crf = config.crf,
            preset = config.preset,
            input_path = input.display(),
        )
    );

    let mut x265_builder = crate::tool_builders::X265Builder::new();
    x265_builder.y4m(true).input(input).output(hevc_output);
    config.configure_x265_builder(&mut x265_builder, "error");

    let output = x265_builder
        .build()
        .output()
        .context("Failed to run x265")?;

    let duration = start_time.elapsed();
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        crate::log_error!(
            crate::infra::static_logs::messages::LABEL_ENCODER,
            format!(
                "Encoder Audit: Direct encoding failed (exit={exit:?}, elapsed={elapsed:.2}s): {err_msg}",
                exit = output.status.code(),
                elapsed = duration.as_secs_f64(),
                err_msg = format_ffmpeg_error(&stderr),
            )
        );
        if !stderr.is_empty() {
            crate::media_conversion_gate::delivery_encode_batch_audit(
                "delivery_encode",
                format!("x265 stderr:\n{stderr}"),
            );
        }
        bail!(
            "x265 encode failed with exit code {:?}",
            output.status.code()
        );
    }

    crate::log_detail!(
        &crate::infra::static_logs::messages::MSG_ENCODE_DIRECT_AUDIT
            .replace("{}", &format!("{:.2}", duration.as_secs_f64()))
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
                            crate::log_detail!(&format!(" [{label}] {line}"));
                        }
                    }

                    if output.len() + line.len() + 1 > MAX_TOTAL_LOG_SIZE {
                        break;
                    }
                    output.push_str(&line);
                    output.push('\n');
                }
                Err(err) => {
                    crate::media_conversion_gate::delivery_encode_batch_audit(
                        "delivery_encode",
                        format!("Encoder Audit: Failed to join log thread ({label}): {err}"),
                    );
                    let _ = writeln!(output, "[stderr read error: {err}]");
                    break;
                }
            }
        }
        output
    })
}

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
    x265_builder.y4m(true).input("-").output(hevc_output);
    config.configure_x265_builder(&mut x265_builder, log_level);

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

        let copy_result: Result<Result<u64, std::io::Error>, _> = transfer_thread.join();
        let pipe_io_error = match &copy_result {
            Ok(Err(io_err)) => Some(io_err),
            Ok(Ok(_)) => None,
            Err(_join_err) => None,
        };
        let is_broken_pipe = pipe_io_error.is_some_and(|e| {
            use std::io::ErrorKind;
            matches!(e.kind(), ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
        });

        let x265_status = x265_child.wait().context("Failed to wait for x265")?;
        let ffmpeg_status = ffmpeg_child.wait().context("Failed to wait for ffmpeg")?;

        let duration = start_time.elapsed();

        let ffmpeg_stderr = match ffmpeg_stderr_thread {
            Some(handle) => handle.join().map_err(|join_err| {
                anyhow::anyhow!("ffmpeg decode stderr thread panicked: {join_err:?}")
            })?,
            None => String::new(),
        };
        let x265_stderr = match x265_stderr_thread {
            Some(handle) => handle
                .join()
                .map_err(|join_err| anyhow::anyhow!("x265 stderr thread panicked: {join_err:?}"))?,
            None => String::new(),
        };

        if !ffmpeg_status.success() {
            crate::log_error!(
                crate::infra::static_logs::messages::LABEL_ENCODER,
                format!(
                    "Encoder Audit: Decoder pipe failed (exit={exit:?}, elapsed={elapsed:.2}s, broken_pipe={is_broken_pipe})",
                    exit = ffmpeg_status.code(),
                    elapsed = duration.as_secs_f64(),
                )
            );
            if is_broken_pipe {
                crate::log_pipeline_broken!(
                    "FFmpeg Decoding",
                    "Reader (x265) likely closed stdin first; x265 may have exited or rejected the stream"
                );
                if !x265_stderr.is_empty() {
                    crate::media_conversion_gate::delivery_encode_batch_audit(
                        "delivery_encode",
                        format!("x265 stderr (often shows why pipe closed):\n{x265_stderr}"),
                    );
                }
            }
            if !ffmpeg_stderr.is_empty() {
                crate::media_conversion_gate::delivery_encode_batch_audit(
                    "delivery_encode",
                    format!("FFmpeg error output:\n{ffmpeg_stderr}"),
                );
            }
            bail!(
                "FFmpeg decode failed (exit_code: {:?})\n\nStderr:\n{}",
                ffmpeg_status.code(),
                ffmpeg_stderr.trim()
            );
        }

        if !x265_status.success() {
            crate::log_error!(
                crate::infra::static_logs::messages::LABEL_ENCODER,
                format!(
                    "Encoder Audit: Pipe establishment failed (exit={exit:?}, elapsed={elapsed:.2}s, broken_pipe={is_broken_pipe})",
                    exit = x265_status.code(),
                    elapsed = duration.as_secs_f64(),
                )
            );
            if is_broken_pipe {
                crate::log_pipeline_broken!(
                    "x265 Encoding",
                    "Encoder (x265) likely exited first; check x265 stderr output for details"
                );
            }
            if !x265_stderr.is_empty() {
                crate::media_conversion_gate::delivery_encode_batch_audit(
                    "delivery_encode",
                    format!("x265 error output:\n{x265_stderr}"),
                );
            }
            bail!("x265 encode failed with exit code {:?}", x265_status.code());
        }

        match copy_result {
            Ok(Err(io_err)) => {
                crate::log_error!(
                    crate::infra::static_logs::messages::LABEL_ENCODER,
                    format!(
                        "Encoder Audit: Pipe copy failed ({io_err}, kind={kind:?})",
                        kind = io_err.kind(),
                    )
                );
                if is_broken_pipe {
                    bail!("Pipe broken during copy (ffmpeg→x265): {io_err}");
                }
                bail!("Pipe I/O error: {io_err}");
            }
            Err(join_err) => {
                crate::log_error!(
                    crate::infra::static_logs::messages::LABEL_ENCODER,
                    &crate::infra::static_logs::messages::MSG_ENCODE_CONCURRENCY_FAIL.replacen(
                        TOKEN_DEBUG,
                        &format!("{join_err:?}"),
                        1
                    )
                );
                bail!("Pipe copy thread panicked: {join_err:?}");
            }
            Ok(Ok(_bytes_copied)) => {}
        }

        crate::log_detail!(
            &crate::infra::static_logs::messages::MSG_ENCODE_PIPED_AUDIT
                .replace("{}", &format!("{:.2}", duration.as_secs_f64()))
        );

        Ok(true)
    } else {
        crate::log_error!(
            crate::infra::static_logs::messages::LABEL_ENCODER,
            crate::infra::static_logs::messages::MSG_ENCODE_IPC_FAIL
        );
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
    let ext = crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path);
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

    crate::log_debug!(
        crate::infra::static_logs::messages::LABEL_ENCODER,
        &crate::infra::static_logs::messages::MSG_MUX_START.replace("{}", &config.container)
    );

    let (status, stderr) = mux_builder
        .output(output)
        .spawn()
        .context("Failed to execute ffmpeg mux")?
        .wait_with_output()
        .context("Failed to wait for ffmpeg mux")?;

    let duration = start_time.elapsed();

    if !status.success() {
        crate::log_error!(
            crate::infra::static_logs::messages::LABEL_ENCODER,
            format!(
                "Muxing Audit: Failed (exit={exit:?}, elapsed={elapsed:.2}s): {err_msg}",
                exit = status.code(),
                elapsed = duration.as_secs_f64(),
                err_msg = format_ffmpeg_error(&stderr),
            )
        );
        bail!("FFmpeg mux failed: {stderr}");
    }

    crate::log_detail!(format!(
        "Muxing Audit: Finalized (elapsed={elapsed:.2}s, container={container})",
        elapsed = duration.as_secs_f64(),
        container = config.container,
    ));

    Ok(())
}

#[must_use]
pub fn is_x265_available() -> bool {
    let result = crate::tool_builders::X265Builder::check_available();

    if result {
        crate::log_debug!(
            crate::infra::static_logs::messages::LABEL_ENCODER,
            crate::infra::static_logs::messages::MSG_ENCODER_AVAILABLE
        );
    } else {
        crate::media_conversion_gate::delivery_encode_batch_audit(
            "delivery_encode",
            crate::infra::static_logs::messages::MSG_ENCODER_MISSING,
        );
    }

    result
}

fn format_ffmpeg_error(stderr: &str) -> String {
    crate::media_conversion_gate::encode_stderr_last_line_or_unknown(
        stderr,
        "ffmpeg_mux",
        "x265 mux stderr",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn collect_x265_args(config: &X265Config) -> Vec<String> {
        let mut builder = crate::tool_builders::X265Builder::new();
        builder
            .y4m(true)
            .input(Path::new("input.y4m"))
            .output(Path::new("output.hevc"));
        config.configure_x265_builder(&mut builder, "error");
        builder
            .build()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn test_x265_available() {
        let _ = is_x265_available();
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

    #[test]
    fn x265_color_signaling_preserves_bt2020_sdr_without_hdr10_opt() {
        let args = collect_x265_args(&X265Config {
            pix_fmt: "yuv420p10le".to_string(),
            color_primaries: Some("bt2020".to_string()),
            color_trc: Some("bt709".to_string()),
            colorspace: Some("bt2020nc".to_string()),
            ..Default::default()
        });

        assert!(!args.iter().any(|arg| arg == "--hdr10-opt"));
        assert!(!args.iter().any(|arg| arg == "--hdr10"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--colorprim", "bt2020"])
        );
        assert!(args.windows(2).any(|pair| pair == ["--transfer", "bt709"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--colormatrix", "bt2020nc"])
        );
        assert!(args.iter().any(|arg| arg == "--repeat-headers"));
    }

    #[test]
    fn x265_color_signaling_enables_hdr10_opt_only_for_hdr10_signaling() {
        let args = collect_x265_args(&X265Config {
            pix_fmt: "yuv420p10le".to_string(),
            color_primaries: Some("bt2020".to_string()),
            color_trc: Some(crate::constants::HDR_TRANSFER_PQ.to_string()),
            colorspace: Some("bt2020nc".to_string()),
            mastering_display: Some(
                "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)".to_string(),
            ),
            max_cll: Some("1000,400".to_string()),
            ..Default::default()
        });

        assert!(args.iter().any(|arg| arg == "--hdr10"));
        assert!(args.iter().any(|arg| arg == "--hdr10-opt"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--colorprim", "bt2020"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--transfer", "smpte2084"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--colormatrix", "bt2020nc"])
        );
        assert!(args.windows(2).any(|pair| pair
            == [
                "--master-display",
                "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)"
            ]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--max-cll", "1000,400"])
        );
        assert!(args.iter().any(|arg| arg == "--repeat-headers"));
    }

    #[test]
    fn x265_color_signaling_does_not_enable_hdr10_opt_for_hlg() {
        let args = collect_x265_args(&X265Config {
            pix_fmt: "yuv420p10le".to_string(),
            color_primaries: Some("bt2020".to_string()),
            color_trc: Some(crate::constants::HDR_TRANSFER_HLG.to_string()),
            colorspace: Some("bt2020nc".to_string()),
            ..Default::default()
        });

        assert!(!args.iter().any(|arg| arg == "--hdr10-opt"));
        assert!(!args.iter().any(|arg| arg == "--hdr10"));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--colorprim", "bt2020"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--transfer", "arib-std-b67"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--colormatrix", "bt2020nc"])
        );
        assert!(args.iter().any(|arg| arg == "--repeat-headers"));
    }

    #[test]
    fn x265_color_signaling_drops_static_hdr10_metadata_for_hlg() {
        let args = collect_x265_args(&X265Config {
            pix_fmt: "yuv420p10le".to_string(),
            color_primaries: Some("bt2020".to_string()),
            color_trc: Some(crate::constants::HDR_TRANSFER_HLG.to_string()),
            colorspace: Some("bt2020nc".to_string()),
            mastering_display: Some(
                "G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,1)".to_string(),
            ),
            max_cll: Some("1000,400".to_string()),
            ..Default::default()
        });

        assert!(!args.iter().any(|arg| arg == "--hdr10-opt"));
        assert!(!args.iter().any(|arg| arg == "--hdr10"));
        assert!(!args.iter().any(|arg| arg == "--master-display"));
        assert!(!args.iter().any(|arg| arg == "--max-cll"));
    }

    #[test]
    fn x265_color_signaling_for_hdr10plus_without_static_metadata_sets_hdr10() {
        let args = collect_x265_args(&X265Config {
            pix_fmt: "yuv420p10le".to_string(),
            color_primaries: Some("bt2020".to_string()),
            color_trc: Some(crate::constants::HDR_TRANSFER_PQ.to_string()),
            colorspace: Some("bt2020nc".to_string()),
            x265_params: Some("dhdr10-info=/tmp/hdr10plus.json".to_string()),
            ..Default::default()
        });

        assert!(args.iter().any(|arg| arg == "--hdr10"));
        assert!(args.iter().any(|arg| arg == "--hdr10-opt"));
        assert!(
            args.iter()
                .any(|arg| arg == "--dhdr10-info=/tmp/hdr10plus.json")
        );
        assert!(!args.iter().any(|arg| arg == "--master-display"));
        assert!(!args.iter().any(|arg| arg == "--max-cll"));
    }
}

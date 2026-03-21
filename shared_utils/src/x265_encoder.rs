//! x265 Direct CPU Encoder Module
//!
//! 🔥 v6.9.17: CPU编码架构 - 使用x265命令行工具直接编码
//!
//! ## 架构设计
//!
//! 由于系统FFmpeg缺少libx265支持，采用三步编码流程：
//! 1. FFmpeg解码 → Y4M (raw YUV)
//! 2. x265编码 → HEVC bitstream
//! 3. FFmpeg封装 → MP4容器
//!
//! ## 优势
//! - 不依赖FFmpeg编译选项
//! - 完整的CRF控制（sub-integer精度）
//! - 更高的SSIM质量（≥0.98 vs VideoToolbox ~0.95）
//! - 严格的CPU编码路径（无GPU fallback）

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, error, warn};

#[derive(Debug, Clone)]
pub struct X265Config {
    pub crf: f32,
    pub preset: String,
    pub threads: usize,
    pub container: String,
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
}

impl Default for X265Config {
    fn default() -> Self {
        Self {
            crf: 23.0,
            preset: "medium".to_string(),
            threads: crate::thread_manager::get_optimal_threads(),
            container: "mp4".to_string(),
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
        }
    }
}

pub fn encode_with_x265(
    input: &Path,
    output: &Path,
    config: &X265Config,
    vf_args: &[String],
) -> Result<u64> {
    debug!(
        "🖥️ CPU encoding started: CRF {:.1}, preset={}",
        config.crf, config.preset
    );

    use crate::universal_heartbeat::{HeartbeatConfig, HeartbeatGuard};
    let _heartbeat = HeartbeatGuard::new(
        HeartbeatConfig::medium("x265 CLI Encoding").with_info(format!("CRF {:.1}", config.crf)),
    );

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

/// Encode a .y4m file directly with x265 (no FFmpeg pipe). Avoids Broken pipe when
/// the pipeline FFmpeg stdout → x265 stdin is used with low-fps or odd y4m streams.
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

    let output = Command::new("x265")
        .arg("--y4m")
        .arg("--input")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("--output")
        .arg(crate::safe_path_arg(hevc_output).as_ref())
        .arg("--crf")
        .arg(format!("{:.1}", config.crf))
        .arg("--preset")
        .arg(&config.preset)
        .arg("--pools")
        .arg(config.threads.to_string())
        .arg("--log-level")
        .arg("error")
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
            eprintln!("x265 stderr:\n{}", stderr);
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
        .map(|e| e.eq_ignore_ascii_case("y4m"))
        .unwrap_or(false);
    if is_y4m {
        return encode_y4m_direct(input, hevc_output, config, start_time);
    }

    let mut ffmpeg_cmd = Command::new("ffmpeg");
    ffmpeg_cmd
        .arg("-y")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-f")
        .arg("yuv4mpegpipe");

    for arg in vf_args {
        ffmpeg_cmd.arg(arg);
    }

    ffmpeg_cmd
        .arg("-pix_fmt")
        .arg(&config.pix_fmt)
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut x265_cmd = Command::new("x265");
    x265_cmd
        .arg("--y4m")
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(crate::safe_path_arg(hevc_output).as_ref())
        .arg("--crf")
        .arg(format!("{:.1}", config.crf))
        .arg("--preset")
        .arg(&config.preset)
        .arg("--pools")
        .arg(config.threads.to_string())
        .arg("--log-level")
        .arg("error");

    // HDR-specific x265 options: enabled when the source is 10-bit or has explicit HDR metadata.
    let is_hdr_content = config.pix_fmt.contains("10")
        || config.mastering_display.is_some()
        || config.max_cll.is_some()
        || matches!(
            config.color_trc.as_deref(),
            Some("smpte2084") | Some("arib-std-b67")
        );
    if is_hdr_content {
        x265_cmd.arg("--hdr10-opt").arg("--repeat-headers");

        if let Some(ref cp) = config.color_primaries {
            x265_cmd.arg("--colorprim").arg(cp);
        }
        if let Some(ref trc) = config.color_trc {
            x265_cmd.arg("--transfer").arg(trc);
        }
        if let Some(ref cs) = config.colorspace {
            x265_cmd.arg("--colormatrix").arg(cs);
        }
        if let Some(ref md) = config.mastering_display {
            x265_cmd.arg("--master-display").arg(md);
        }
        if let Some(ref cll) = config.max_cll {
            x265_cmd.arg("--max-cll").arg(cll);
        }
    }

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

    let ffmpeg_stderr_thread = ffmpeg_child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Read};
            let reader = BufReader::with_capacity(8192, stderr.take(10 * 1024 * 1024));
            let mut output = String::with_capacity(64 * 1024);
            for line in reader.lines().take(100_000) {
                match line {
                    Ok(line) => {
                        if output.len() + line.len() + 1 > 1024 * 1024 {
                            break;
                        }
                        output.push_str(&line);
                        output.push('\n');
                    }
                    Err(err) => {
                        warn!("Failed to read ffmpeg decode stderr: {}", err);
                        output.push_str(&format!("[stderr read error: {}]\n", err));
                        break;
                    }
                }
            }
            output
        })
    });

    let x265_stderr_thread = x265_child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Read};
            let reader = BufReader::with_capacity(8192, stderr.take(10 * 1024 * 1024));
            let mut output = String::with_capacity(64 * 1024);
            for line in reader.lines().take(100_000) {
                match line {
                    Ok(line) => {
                        if output.len() + line.len() + 1 > 1024 * 1024 {
                            break;
                        }
                        output.push_str(&line);
                        output.push('\n');
                    }
                    Err(err) => {
                        warn!("Failed to read x265 stderr: {}", err);
                        output.push_str(&format!("[stderr read error: {}]\n", err));
                        break;
                    }
                }
            }
            output
        })
    });

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
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let x265_stderr = x265_stderr_thread
            .and_then(|h| h.join().ok())
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
                warn!("Pipe broken: reader (x265) likely closed stdin first; x265 may have exited or rejected the stream");
                if !x265_stderr.is_empty() {
                    eprintln!(
                        "x265 stderr (often shows why pipe closed):\n{}",
                        x265_stderr
                    );
                }
            }
            if !ffmpeg_stderr.is_empty() {
                eprintln!("FFmpeg error output:\n{}", ffmpeg_stderr);
            }
            bail!("FFmpeg decode failed");
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
                eprintln!("x265 error output:\n{}", x265_stderr);
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
                bail!("Pipe broken during copy (ffmpeg→x265): {}", io_err);
            }
            bail!("Pipe I/O error: {}", io_err);
        }
        if let Err(join_err) = copy_result {
            error!("Pipe copy thread panicked: {:?}", join_err);
            bail!("Pipe copy thread panicked: {:?}", join_err);
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

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-i")
        .arg(crate::safe_path_arg(hevc_file).as_ref());

    if config.preserve_audio && !input_is_image {
        cmd.arg("-i")
            .arg(crate::safe_path_arg(original_input).as_ref());
        // Map: video from HEVC bitstream (input 0), all audio + subtitle from original (input 1)
        cmd.arg("-map").arg("0:v:0");
        cmd.arg("-map").arg("1:a?");
        cmd.arg("-c:v").arg("copy");

        // Audio: copy when compatible, transcode only for incompatible codecs
        let audio_args =
            crate::audio_args_for_container(config.audio_codec.as_deref(), &config.container);
        for arg in &audio_args {
            // Skip -an since we already have -map 1:a?
            if arg != "-an" {
                cmd.arg(arg);
            }
        }

        // Subtitles: map and copy/transcode as appropriate for container
        if config.has_subtitles {
            cmd.arg("-map").arg("1:s?");
            let sub_args = crate::subtitle_args_for_container(
                true,
                config.subtitle_codec.as_deref(),
                &config.container,
            );
            for arg in sub_args {
                cmd.arg(arg);
            }
        }
    } else {
        // No audio: either disabled or source is an image format with no audio streams.
        cmd.arg("-c:v").arg("copy").arg("-an");
    }

    if config.container == "mp4" || config.container == "mov" {
        cmd.arg("-tag:v").arg("hvc1");
        cmd.arg("-movflags").arg("+faststart");
    }

    cmd.arg(crate::safe_path_arg(output).as_ref())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

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
        bail!("FFmpeg mux failed: {}", stderr);
    }

    debug!(
        duration_secs = duration.as_secs_f64(),
        output_file = ?output,
        "FFmpeg mux completed successfully"
    );

    Ok(())
}

pub fn is_x265_available() -> bool {
    let result = Command::new("x265")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

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

    #[test]
    fn test_x265_available() {
        if is_x265_available() {
            println!("✅ x265 is available");
        } else {
            println!("⚠️  x265 not found - install with: brew install x265");
        }
    }
}

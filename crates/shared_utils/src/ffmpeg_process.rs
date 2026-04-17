//! FFmpeg Process Management Module - Deadlock Prevention
//!
//! ## Problem Background
//!
//! When both stdout and stderr are piped but only stdout is read, if `FFmpeg` outputs a large amount of
//! stderr logs (exceeding the 64KB buffer), a deadlock occurs:
//! - `FFmpeg` blocks due to a full stderr buffer
//! - The Rust program blocks while waiting for stdout
//! - Both wait for each other, and the program freezes
//!
//! ## Solution
//!
//! Use a separate thread to concurrently consume stderr, ensuring the buffer never fills up.
//!
//! ## Usage Example
//!
//! ```ignore
//! use shared_utils::ffmpeg_process::FfmpegProcess;
//! use std::process::Command;
//!
//! let mut cmd = Command::new("ffmpeg");
//! cmd.arg("-i").arg("input.mp4").arg("output.mp4");
//!
//! let mut process = FfmpegProcess::spawn(&mut cmd)?;
//!
//! // Read stdout progress
//! if let Some(stdout) = process.stdout() {
//!     // Handle progress...
//! }
//!
//! // Wait for completion
//! let (status, stderr) = process.wait_with_output()?;
//! ```

use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use tracing::{debug, error, info, warn};

pub struct FfmpegProcess {
    child: Child,
    stderr_thread: Option<JoinHandle<String>>,
}

impl FfmpegProcess {
    /// Spawn a new `FFmpeg` process.
    ///
    /// # Errors
    /// Returns error if spawning fails or stderr cannot be captured.
    pub fn spawn(cmd: &mut Command) -> Result<Self> {
        let command_str = format!("{cmd:?}");
        info!(
            command = %command_str,
            "Executing FFmpeg command"
        );

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn FFmpeg process")?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture FFmpeg stderr"))?;

        let stderr_thread = thread::spawn(move || {
            let mut buf = String::new();
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if crate::progress_mode::is_verbose_mode() {
                            crate::progress_mode::emit_stderr(&format!("   [ffmpeg] {line}"));
                        }
                        buf.push_str(&line);
                        buf.push('\n');
                    }
                    Err(err) => {
                        error!("Failed to read FFmpeg stderr: {err}");
                        let _ = writeln!(buf, "[stderr read error: {err}]");
                        break;
                    }
                }
            }
            buf
        });

        Ok(Self {
            child,
            stderr_thread: Some(stderr_thread),
        })
    }

    pub const fn stdout(&mut self) -> Option<&mut ChildStdout> {
        self.child.stdout.as_mut()
    }

    pub const fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// Wait for `FFmpeg` process to complete and return its output.
    ///
    /// # Errors
    /// Returns error if waiting for child process fails.
    pub fn wait_with_output(mut self) -> Result<(ExitStatus, String)> {
        // If caller never took stdout, drain it in background so FFmpeg does not block on write (pipe buffer full).
        let stdout_drain = self.child.stdout.take().map(|stdout| {
            thread::spawn(move || {
                use std::io::Read;
                let mut reader = BufReader::new(stdout);
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => return None::<String>,
                        Ok(_) => {}
                        Err(err) => return Some(err.to_string()),
                    }
                }
            })
        });
        let status = self.child.wait().context("Failed to wait for FFmpeg")?;
        if let Some(h) = stdout_drain {
            match h.join() {
                Ok(Some(err)) => warn!(error = %err, "Failed while draining FFmpeg stdout"),
                Ok(None) => {}
                Err(_) => warn!("FFmpeg stdout drain thread panicked"),
            }
        }
        let stderr = self.stderr_thread.take().map_or_else(String::new, |t| {
            t.join().unwrap_or_else(|_| {
                warn!("FFmpeg stderr reader thread panicked");
                String::new()
            })
        });

        if status.success() {
            info!(
                exit_code = status.code(),
                "FFmpeg process completed successfully"
            );
            debug!(
                stderr_output = %stderr,
                "FFmpeg stderr output"
            );
        } else {
            error!(
                exit_code = status.code(),
                stderr_output = %stderr,
                "FFmpeg process failed"
            );
        }

        Ok((status, stderr))
    }

    /// Check if process has finished.
    ///
    /// # Errors
    /// Returns error if checking status fails.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("Failed to check FFmpeg status")
    }

    /// Kill the process.
    ///
    /// # Errors
    /// Returns error if killing fails.
    pub fn kill(&mut self) -> Result<()> {
        self.child.kill().context("Failed to kill FFmpeg process")
    }
}

#[derive(Debug, Clone)]
pub struct FfmpegProgressParser {
    total_frames: Option<u64>,
    total_duration: Option<f64>,
    current_frame: u64,
    current_time: f64,
    current_fps: f64,
    current_speed: f64,
}

impl FfmpegProgressParser {
    #[must_use]
    pub const fn new(total_frames: Option<u64>) -> Self {
        Self {
            total_frames,
            total_duration: None,
            current_frame: 0,
            current_time: 0.0,
            current_fps: 0.0,
            current_speed: 0.0,
        }
    }

    #[must_use]
    pub const fn with_duration(total_duration: f64) -> Self {
        Self {
            total_frames: None,
            total_duration: Some(total_duration),
            current_frame: 0,
            current_time: 0.0,
            current_fps: 0.0,
            current_speed: 0.0,
        }
    }

    pub fn parse_line(&mut self, line: &str) -> Option<f64> {
        if let Some(frame_str) = line.strip_prefix("frame=") {
            if let Ok(frame) = frame_str.split_whitespace().next()?.parse::<u64>() {
                self.current_frame = frame;
            }
        }

        if let Some(fps_str) = line.strip_prefix("fps=") {
            if let Ok(fps) = fps_str.split_whitespace().next()?.parse::<f64>() {
                self.current_fps = fps;
            }
        }

        if let Some(time_str) = line.strip_prefix("time=") {
            if let Some(time) = Self::parse_time(time_str.split_whitespace().next()?) {
                self.current_time = time;
            }
        }

        if let Some(speed_str) = line.strip_prefix("speed=") {
            let speed_str = speed_str.trim().trim_end_matches('x');
            if let Ok(speed) = speed_str.parse::<f64>() {
                self.current_speed = speed;
            }
        }

        self.calculate_progress()
    }

    fn parse_time(time_str: &str) -> Option<f64> {
        let parts: Vec<&str> = time_str.split(':').collect();
        if parts.len() != 3 {
            return None;
        }

        let hours: f64 = parts[0].parse().ok()?;
        let minutes: f64 = parts[1].parse().ok()?;
        let seconds: f64 = parts[2].parse().ok()?;

        Some(hours.mul_add(3600.0, minutes.mul_add(60.0, seconds)))
    }

    fn calculate_progress(&self) -> Option<f64> {
        if let Some(total) = self.total_frames {
            if total > 0 && self.current_frame > 0 {
                #[allow(clippy::cast_precision_loss)]
                return Some((self.current_frame as f64 / total as f64).min(1.0));
            }
        }

        if let Some(total) = self.total_duration {
            if total > 0.0 && self.current_time > 0.0 {
                return Some((self.current_time / total).min(1.0));
            }
        }

        None
    }

    #[must_use]
    pub const fn current_frame(&self) -> u64 {
        self.current_frame
    }

    #[must_use]
    pub const fn current_time(&self) -> f64 {
        self.current_time
    }

    #[must_use]
    pub const fn current_fps(&self) -> f64 {
        self.current_fps
    }

    #[must_use]
    pub const fn current_speed(&self) -> f64 {
        self.current_speed
    }
}

#[must_use]
pub fn format_ffmpeg_error(stderr: &str) -> String {
    if let Some(error_line) = stderr
        .lines()
        .rev()
        .find(|line| line.contains("Error") || line.contains("error"))
    {
        return error_line.trim().to_string();
    }

    stderr
        .lines()
        .rev()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("frame=")
                && !trimmed.starts_with("fps=")
                && !trimmed.starts_with("size=")
        })
        .map_or_else(
            || "Unknown FFmpeg error".to_string(),
            |s| s.trim().to_string(),
        )
}

#[must_use]
pub fn is_recoverable_error(stderr: &str) -> bool {
    let recoverable_patterns = [
        "Resource temporarily unavailable",
        "Cannot allocate memory",
        "Too many open files",
        "Connection reset",
        "Broken pipe",
    ];
    recoverable_patterns
        .iter()
        .any(|pattern| stderr.contains(pattern))
}

#[derive(Debug, Clone)]
pub struct FfmpegError {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub suggestion: Option<String>,
}

impl std::fmt::Display for FfmpegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "❌ FFMPEG ERROR")?;
        writeln!(f, "   Command: {}", self.command)?;
        if let Some(code) = self.exit_code {
            writeln!(f, "   Exit code: {code}")?;
        }
        writeln!(f, "   Error: {}", format_ffmpeg_error(&self.stderr))?;
        if let Some(ref suggestion) = self.suggestion {
            writeln!(f, "   💡 Suggestion: {suggestion}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FfmpegError {}

#[must_use]
pub fn get_error_suggestion(stderr: &str) -> Option<String> {
    let patterns = [
        ("No such file or directory", "Check input file path"),
        (
            "Invalid data found",
            "Input file may be corrupted; try re-downloading",
        ),
        ("Encoder", "Install encoder (e.g. libx265, libsvtav1)"),
        ("not found", "Check that FFmpeg is installed correctly"),
        ("Permission denied", "Check file permissions (read/write)"),
        (
            "Output file is empty",
            "Encode failed; try lowering quality parameters",
        ),
        ("Avi header", "AVI header corrupted; try -fflags +genpts"),
        (
            "moov atom not found",
            "MP4 file incomplete; try -movflags faststart",
        ),
        (
            "Invalid NAL unit size",
            "Video stream corrupted; try -err_detect ignore_err",
        ),
        (
            "Discarding",
            "Some frames discarded; possible timestamp issue",
        ),
        (
            "Too many packets buffered",
            "Increase -max_muxing_queue_size",
        ),
    ];

    for (pattern, suggestion) in patterns {
        if stderr.contains(pattern) {
            return Some((*suggestion).to_string());
        }
    }
    None
}

/// Run `FFmpeg` command and report error details if it fails.
///
/// # Errors
/// Returns error if command fails, also prints error details to stderr.
pub fn run_ffmpeg_with_error_report(args: &[&str]) -> Result<std::process::Output> {
    let mut builder = crate::tool_builders::FfmpegBuilder::new();
    for arg in args {
        builder.arg(arg);
    }

    let command_str = format!("{} {}", crate::constants::TOOL_FFMPEG, args.join(" "));

    info!(
        command = %command_str,
        "Executing FFmpeg command"
    );

    let output = builder
        .build()
        .output()
        .context("Failed to execute FFmpeg")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        let error = FfmpegError {
            command: command_str,
            stdout,
            stderr,
            exit_code: output.status.code(),
            suggestion: get_error_suggestion(String::from_utf8_lossy(&output.stderr).as_ref()),
        };

        error!(
            command = %error.command,
            exit_code = ?error.exit_code,
            stderr = %error.stderr,
            stdout = %error.stdout,
            suggestion = ?error.suggestion,
            "FFmpeg command failed"
        );

        eprintln!("{error}");

        return Err(anyhow::anyhow!(error));
    }

    info!(
        exit_code = output.status.code(),
        "FFmpeg command completed successfully"
    );
    debug!(
        stdout_length = output.stdout.len(),
        stderr_length = output.stderr.len(),
        "FFmpeg output captured"
    );

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_ffmpeg_error_with_error_line() {
        let stderr = r"
frame=  100 fps=25.0 q=28.0 size=    1024kB time=00:00:04.00 bitrate=2097.2kbits/s
[libx265 @ 0x7f8b8c000000] Error: invalid parameter
";
        let error = format_ffmpeg_error(stderr);
        assert!(error.contains("Error"));
        assert!(error.contains("invalid parameter"));
    }

    #[test]
    fn test_format_ffmpeg_error_no_error_line() {
        let stderr = r"
frame=  100 fps=25.0 q=28.0 size=    1024kB time=00:00:04.00
Conversion failed!
";
        let error = format_ffmpeg_error(stderr);
        assert_eq!(error, "Conversion failed!");
    }

    #[test]
    fn test_format_ffmpeg_error_empty() {
        let error = format_ffmpeg_error("");
        assert_eq!(error, "Unknown FFmpeg error");
    }

    #[test]
    fn test_progress_parser_frame() {
        let mut parser = FfmpegProgressParser::new(Some(1000));
        let progress = parser.parse_line("frame=  500");
        assert_eq!(progress, Some(0.5));
        assert_eq!(parser.current_frame(), 500);
    }

    #[test]
    fn test_progress_parser_time() {
        let mut parser = FfmpegProgressParser::with_duration(120.0);
        let progress = parser.parse_line("time=00:01:00.00");
        assert_eq!(progress, Some(0.5));
        assert!((parser.current_time() - 60.0).abs() < 0.01);
    }

    #[test]
    fn test_progress_parser_fps() {
        let mut parser = FfmpegProgressParser::new(None);
        parser.parse_line("fps=29.97");
        assert!((parser.current_fps() - 29.97).abs() < 0.01);
    }

    #[test]
    fn test_is_recoverable_error() {
        assert!(is_recoverable_error("Resource temporarily unavailable"));
        assert!(is_recoverable_error("Cannot allocate memory"));
        assert!(!is_recoverable_error("Invalid input file"));
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_progress_parser_frame_accuracy(
            current in 0u64..10000,
            total in 1_u64..10000
        ) {
            let mut parser = FfmpegProgressParser::new(Some(total));
            let line = format!("frame={current}");
            let progress = parser.parse_line(&line);

            if current > 0 {
                let expected = (current as f64 / total as f64).min(1.0);
                prop_assert!(progress.is_some());
                let actual = progress.unwrap();
                prop_assert!((actual - expected).abs() < 0.001,
                    "Expected {}, got {} for frame {}/{}", expected, actual, current, total);
            }
        }

        #[test]
        fn prop_progress_parser_time_accuracy(
            hours in 0u32..24,
            minutes in 0u32..60,
            seconds in 0u32..60,
            total_duration in 1.0f64..86400.0
        ) {
            let mut parser = FfmpegProgressParser::with_duration(total_duration);
            let line = format!("time={hours:02}:{minutes:02}:{seconds:02}.00");
            let progress = parser.parse_line(&line);

            let current_seconds = f64::from(hours).mul_add(3600.0, f64::from(minutes) * 60.0) + f64::from(seconds);
            if current_seconds > 0.0 {
                let expected = (current_seconds / total_duration).min(1.0);
                prop_assert!(progress.is_some());
                let actual = progress.unwrap();
                prop_assert!((actual - expected).abs() < 0.01,
                    "Expected {}, got {} for time {}:{}:{}", expected, actual, hours, minutes, seconds);
            }
        }

        #[test]
        fn prop_format_error_non_empty(
            content in "[a-zA-Z0-9 ]{1,100}"
        ) {
            let error = format_ffmpeg_error(&content);
            prop_assert!(!error.is_empty(), "Error message should not be empty");
        }

        #[test]
        fn prop_format_error_prefers_error_line(
            prefix in "[a-zA-Z ]{0,50}",
            suffix in "[a-zA-Z ]{0,50}"
        ) {
            let stderr = format!("{prefix}\nError: test error message\n{suffix}");
            let error = format_ffmpeg_error(&stderr);
            prop_assert!(error.contains("Error"),
                "Should contain 'Error', got: {}", error);
        }
    }
}

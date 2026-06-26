//! Process streaming with stats parsing.
//! Mirrors `stream_and_log_process()` from drag_and_drop_processor.py.

use crate::infra::hardening::delegated_exit_code;
use anyhow::{Context, Result, bail};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Statistics collected from process output.
#[derive(Debug, Clone, Default)]
pub struct ProcessorStats {
    pub succeeded: usize,
    pub skipped: usize,
    pub ignored: usize,
    pub failed: usize,
    pub exit_code: i32,
}

impl ProcessorStats {
    #[must_use]
    pub fn total(&self) -> usize {
        self.succeeded + self.skipped + self.ignored + self.failed
    }
}

fn parse_stats_count(token: &str) -> Option<usize> {
    match token.parse::<usize>() {
        Ok(n) => Some(n),
        Err(err) => {
            eprintln!("[PROCESS] stats count parse failed for {token:?}: {err}");
            None
        }
    }
}

/// Parse a single stats line; returns true if line was consumed.
pub fn ingest_stats_line(stats: &mut ProcessorStats, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        let is_target = matches!(parts[0], "Succeeded:" | "Skipped:" | "Ignored:" | "Failed:");
        if is_target && let Some(n) = parse_stats_count(parts[1]) {
            match parts[0] {
                "Succeeded:" => stats.succeeded = n,
                "Skipped:" => stats.skipped = n,
                "Ignored:" => stats.ignored = n,
                "Failed:" => stats.failed = n,
                _ => {}
            }
        }
    }
}

fn drain_reader<R: Read, F: FnMut(&str)>(
    reader: R,
    stats: &mut ProcessorStats,
    line_handler: &mut F,
) {
    for line in BufReader::new(reader).lines() {
        match line {
            Ok(l) => {
                ingest_stats_line(stats, &l);
                line_handler(&l);
            }
            Err(err) => eprintln!("[PROCESS] stream read failed: {err}"),
        }
    }
}

/// Parse statistics from output text (mirrors Python parse_processor_stats).
pub fn parse_stats_from_output(output: &str) -> ProcessorStats {
    let mut stats = ProcessorStats::default();
    for line in output.lines() {
        ingest_stats_line(&mut stats, line);
    }
    stats
}

/// Stream child output line-by-line with callback; accumulate ProcessorStats.
pub fn stream_child_output_collecting<F>(
    mut child: std::process::Child,
    mut line_handler: F,
) -> Result<ProcessorStats>
where
    F: FnMut(&str),
{
    let mut stats = ProcessorStats::default();
    if let Some(stdout) = child.stdout.take() {
        drain_reader(stdout, &mut stats, &mut line_handler);
    }
    if let Some(stderr) = child.stderr.take() {
        drain_reader(stderr, &mut stats, &mut line_handler);
    }
    let status = child.wait().context("wait for child")?;
    stats.exit_code = delegated_exit_code(status, "child", "stream_child_output_collecting");
    Ok(stats)
}

/// Stream child process output line-by-line with callback.
pub fn stream_child_output<F>(child: std::process::Child, line_handler: F) -> Result<i32>
where
    F: FnMut(&str),
{
    let stats = stream_child_output_collecting(child, line_handler)?;
    Ok(stats.exit_code)
}

/// Check if PTY streaming is available on this platform.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[must_use]
pub fn pty_available() -> bool {
    true
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[must_use]
pub fn pty_available() -> bool {
    false
}

fn strip_ansi_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1B' && chars.clone().next() == Some('[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn push_line<F: FnMut(&str)>(
    _buffer: &mut String,
    line: &str,
    stats: &mut ProcessorStats,
    line_handler: &mut F,
) {
    let mut line = line.to_string();
    if let Some(pos) = line.rfind('\r') {
        line = line[pos + 1..].to_string();
    }
    let clean = strip_ansi_escapes(&line);
    if clean.trim().is_empty() {
        return;
    }
    ingest_stats_line(stats, &clean);
    line_handler(&clean);
}

fn push_chunk_log_lines<F: FnMut(&str)>(
    buffer: &mut String,
    chunk: &str,
    stats: &mut ProcessorStats,
    line_handler: &mut F,
) {
    buffer.push_str(chunk);
    while let Some(pos) = buffer.find('\n') {
        let line = buffer[..pos].to_string();
        buffer.replace_range(..=pos, "");
        push_line(buffer, &line, stats, line_handler);
    }
}

#[cfg(unix)]
fn pty_winsize() -> libc::winsize {
    let mut winsize: libc::winsize = unsafe { std::mem::zeroed() };
    let lines = match std::env::var("LINES") {
        Ok(raw) => raw.trim().parse::<u16>().unwrap_or(45u16),
        Err(_) => 45u16,
    };
    let columns = match std::env::var("COLUMNS") {
        Ok(raw) => raw.trim().parse::<u16>().unwrap_or(45u16),
        Err(_) => 45u16,
    };
    winsize.ws_row = lines;
    winsize.ws_col = columns;
    winsize.ws_xpixel = 0;
    winsize.ws_ypixel = 0;
    winsize
}

#[cfg(unix)]
fn stream_process_with_pty_unix<F, H>(
    cmd: &[String],
    log_path: Option<&Path>,
    mut line_handler: F,
    mut heartbeat_cb: H,
) -> Result<ProcessorStats>
where
    F: FnMut(&str),
    H: FnMut(),
{
    use std::io;
    use std::os::unix::io::FromRawFd;

    let mut master_fd: libc::c_int = 0;
    let mut slave_fd: libc::c_int = 0;
    let ret = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ret != 0 {
        bail!("openpty failed");
    }

    let winsize = pty_winsize();
    let ioctl_ret = unsafe { libc::ioctl(slave_fd, libc::TIOCSWINSZ, &winsize) };
    if ioctl_ret != 0 {
        eprintln!("[PROCESS] PTY winsize sync failed");
    }

    let stderr_fd = unsafe { libc::dup(slave_fd) };
    if stderr_fd < 0 {
        bail!("dup PTY slave failed");
    }
    let stdout = unsafe { Stdio::from_raw_fd(slave_fd) };
    let stderr = unsafe { Stdio::from_raw_fd(stderr_fd) };

    let mut child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("spawn {}", cmd.join(" ")))?;

    let mut master_file = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let flags = unsafe { libc::fcntl(master_fd, libc::F_GETFL) };
    if flags >= 0 {
        let _ = unsafe { libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    }

    let mut stats = ProcessorStats::default();
    let mut output_tail = String::new();
    let mut log_buffer = String::new();
    let mut last_heartbeat = Instant::now();
    let mut buf = [0u8; 16 * 1024];

    loop {
        if last_heartbeat.elapsed() >= Duration::from_secs(60) {
            heartbeat_cb();
            last_heartbeat = Instant::now();
        }

        match master_file.read(&mut buf) {
            Ok(0) => {
                if child.try_wait().context("check pty child")?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(n) => {
                let chunk = &buf[..n];
                let _ = io::stdout().write_all(chunk);
                let _ = io::stdout().flush();

                let text = String::from_utf8_lossy(chunk);
                output_tail.push_str(&text);
                if output_tail.len() > 50_000 {
                    let mut start = output_tail.len() - 50_000;
                    while !output_tail.is_char_boundary(start) {
                        start += 1;
                    }
                    output_tail = output_tail[start..].to_string();
                }
                if let Some(path) = log_path {
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map(|mut file| {
                            push_chunk_log_lines(&mut log_buffer, &text, &mut stats, &mut |line| {
                                let _ = writeln!(file, "{line}");
                            });
                        });
                } else {
                    push_chunk_log_lines(&mut log_buffer, &text, &mut stats, &mut line_handler);
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if child.try_wait().context("check pty child")?.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(err) if matches!(err.kind(), io::ErrorKind::Interrupted) => {}
            Err(err) => return Err(err).context("read pty child output"),
        }
    }

    if !log_buffer.trim().is_empty() {
        let final_line = log_buffer.clone();
        push_line(&mut log_buffer, &final_line, &mut stats, &mut line_handler);
    }
    for line in output_tail.lines() {
        ingest_stats_line(&mut stats, line);
    }

    let status = child.wait().context("wait for pty child")?;
    stats.exit_code = delegated_exit_code(status, &cmd[0], "stream_process_with_pty");
    Ok(stats)
}

#[cfg(not(unix))]
fn stream_process_with_pty_unix<F, H>(
    cmd: &[String],
    _log_path: Option<&Path>,
    _line_handler: F,
    _heartbeat_cb: H,
) -> Result<ProcessorStats>
where
    F: FnMut(&str),
    H: FnMut(),
{
    bail!(
        "PTY streaming unavailable on this platform: {}",
        cmd.join(" ")
    )
}

/// Stream process output through PTY master/slave pair.
/// Mirrors Python pty.openpty() + os.read() implementation.
pub fn stream_process_with_pty<F, H>(
    cmd: &[String],
    log_path: Option<&Path>,
    line_handler: F,
    heartbeat_cb: H,
) -> Result<ProcessorStats>
where
    F: FnMut(&str),
    H: FnMut(),
{
    if pty_available() {
        return stream_process_with_pty_unix(cmd, log_path, line_handler, heartbeat_cb);
    }
    let child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn process for streaming")?;
    stream_child_output_collecting(child, line_handler)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stats_extracts_numbers() {
        let output = "Succeeded: 10\nSkipped: 2\nIgnored: 3\nFailed: 1";
        let stats = parse_stats_from_output(output);
        assert_eq!(stats.succeeded, 10);
        assert_eq!(stats.skipped, 2);
        assert_eq!(stats.ignored, 3);
        assert_eq!(stats.failed, 1);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn test_pty_available_on_unix() {
        assert!(pty_available());
    }
}

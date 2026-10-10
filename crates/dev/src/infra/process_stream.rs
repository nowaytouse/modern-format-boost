//! Process streaming with stats parsing.
//! Mirrors `stream_and_log_process()` from drag_and_drop_processor.py.

use crate::infra::hardening::delegated_exit_code;
use anyhow::{Context, Result, bail};
use std::io::{self, BufRead, BufReader, Read, Write};
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
    /// Observed counters in succeeded, skipped, ignored, failed order.
    pub reported: [bool; 4],
    /// Files still awaiting a terminal outcome, when the child reported it.
    pub unprocessed: Option<usize>,
    /// Distinguish a malformed pending count from a legacy report without it.
    pub unprocessed_invalid: bool,
    /// Exact current-run measurements; absence is not a zero-byte result.
    pub converted_bytes: Option<ProcessorByteTotals>,
    pub converted_bytes_invalid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorByteTotals {
    pub input_bytes: Option<u64>,
    pub output_bytes: Option<u64>,
}

impl ProcessorByteTotals {
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self {
            input_bytes: self
                .input_bytes
                .zip(other.input_bytes)
                .and_then(|(a, b)| a.checked_add(b)),
            output_bytes: self
                .output_bytes
                .zip(other.output_bytes)
                .and_then(|(a, b)| a.checked_add(b)),
        }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            input_bytes: None,
            output_bytes: None,
        }
    }
}

fn parse_converted_bytes(payload: &str) -> Option<ProcessorByteTotals> {
    if payload.len() > 4096 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    if value.get("schema_version")?.as_u64()? != 1
        || value.get("scope")?.as_str()? != "converted_this_run"
    {
        return None;
    }
    let bytes = |key| {
        let field = value.get(key)?;
        if field.is_null() {
            Some(None)
        } else {
            field.as_u64().map(Some)
        }
    };
    Some(ProcessorByteTotals {
        input_bytes: bytes("input_bytes")?,
        output_bytes: bytes("output_bytes")?,
    })
}

impl ProcessorStats {
    #[must_use]
    pub fn checked_total(&self) -> Option<usize> {
        self.succeeded
            .checked_add(self.skipped)
            .and_then(|count| count.checked_add(self.ignored))
            .and_then(|count| count.checked_add(self.failed))
    }

    #[must_use]
    pub fn counts_complete(&self) -> bool {
        self.reported.iter().all(|reported| *reported)
            && !self.unprocessed_invalid
            && self.checked_total().is_some_and(|total| {
                self.unprocessed
                    .is_none_or(|pending| total.checked_add(pending).is_some())
            })
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.succeeded
            .saturating_add(self.skipped)
            .saturating_add(self.ignored)
            .saturating_add(self.failed)
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

fn is_progress_stat(token: &str, label: &str) -> bool {
    let Some(value) = token
        .strip_prefix(label)
        .and_then(|value| value.strip_prefix(':'))
    else {
        return false;
    };
    let chars = value.chars().collect::<Vec<_>>();
    let mut index = 0;
    let consume_digits = |index: &mut usize| {
        let start = *index;
        while chars.get(*index).is_some_and(char::is_ascii_digit) {
            *index += 1;
        }
        *index > start
    };
    if !consume_digits(&mut index) || !matches!(chars.get(index), Some('✓' | '+')) {
        return false;
    }
    index += 1;
    if index == chars.len() {
        return true;
    }
    if matches!(label, "I" | "V") {
        if !consume_digits(&mut index) {
            return false;
        }
        if matches!(chars.get(index), Some('x' | '✗')) {
            return index + 1 == chars.len();
        }
        if chars.get(index) != Some(&'s') {
            return false;
        }
        index += 1;
        if index == chars.len() {
            return true;
        }
    } else if label != "X" {
        return false;
    }
    consume_digits(&mut index)
        && matches!(chars.get(index), Some('x' | '✗'))
        && index + 1 == chars.len()
}

fn is_valid_counter_suffix(parts: &[&str]) -> bool {
    match parts {
        [] | ["│"] | ["|"] => true,
        ["│", "│", chart, xmp, images, preprocessing] if matches!(*chart, "📊" | "#") => {
            is_progress_stat(xmp, "X")
                && is_progress_stat(images, "I")
                && is_progress_stat(preprocessing, "P")
        }
        ["│", "│", stats @ ..] => is_video_progress_overlay(stats),
        _ => false,
    }
}

fn is_video_progress_overlay(stats: &[&str]) -> bool {
    let labels = stats
        .iter()
        .map(|stat| {
            if stat.starts_with("X:") {
                "X"
            } else if stat.starts_with("V:") {
                "V"
            } else if stat.starts_with("P:") {
                "P"
            } else {
                "?"
            }
        })
        .collect::<Vec<_>>();
    let expected = matches!(
        labels.as_slice(),
        ["V"] | ["V", "P"] | ["X", "V"] | ["X", "V", "P"]
    );
    expected
        && stats
            .iter()
            .zip(labels)
            .all(|(stat, label)| label != "?" && is_progress_stat(stat, label))
}

/// Accept bare counters or the exact report decoration, never a filename/message substring.
pub fn ingest_stats_line(stats: &mut ProcessorStats, line: &str) {
    let clean = strip_ansi_escapes(line);
    let mut text = clean.trim();
    if let Some(payload) = text.strip_prefix("MFB_CONVERTED_BYTES=") {
        if stats.converted_bytes_invalid {
            return;
        }
        if let Some(bytes) = parse_converted_bytes(payload)
            && stats
                .converted_bytes
                .is_none_or(|previous| previous == bytes)
        {
            stats.converted_bytes = Some(bytes);
        } else {
            stats.converted_bytes_invalid = true;
            stats.converted_bytes = None;
            eprintln!(
                "[PROCESS] Invalid or conflicting converted-byte receipt; size totals unavailable"
            );
        }
        return;
    }
    if let Some(body) = text.strip_prefix('|').or_else(|| text.strip_prefix('│')) {
        text = body.trim();
    }
    for prefix in ["[OK]", "[X]", "[skip]", "[ignored]", "✅", "❌", "⏭️", "👻"] {
        if let Some(body) = text.strip_prefix(prefix) {
            text = body.trim();
            break;
        }
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    if let Some(label) = parts.first() {
        let index = match *label {
            "Succeeded:" => Some(0),
            "Skipped:" => Some(1),
            "Ignored:" => Some(2),
            "Failed:" => Some(3),
            _ => None,
        };
        if let Some(index) = index {
            if parts.len() < 2 || !is_valid_counter_suffix(&parts[2..]) {
                stats.reported[index] = false;
            } else if let Some(value) = parse_stats_count(parts[1]) {
                stats.reported[index] = true;
                match index {
                    0 => stats.succeeded = value,
                    1 => stats.skipped = value,
                    2 => stats.ignored = value,
                    3 => stats.failed = value,
                    _ => unreachable!(),
                }
            } else {
                stats.reported[index] = false;
            }
        } else if *label == "Unprocessed:" {
            stats.unprocessed = (parts.len() >= 2 && is_valid_counter_suffix(&parts[2..]))
                .then(|| parse_stats_count(parts[1]))
                .flatten();
            stats.unprocessed_invalid = stats.unprocessed.is_none();
        }
    }
}

fn remember_log_write_error(result: io::Result<()>, first_error: &mut Option<io::Error>) {
    if let Err(error) = result
        && first_error.is_none()
    {
        *first_error = Some(error);
    }
}

fn finish_log_writes(first_error: Option<io::Error>, context: &'static str) -> Result<()> {
    if let Some(error) = first_error {
        return Err(error).context(context);
    }
    Ok(())
}

fn open_stream_log(
    log_path: Option<&Path>,
    context: &'static str,
) -> Result<Option<std::fs::File>> {
    log_path
        .map(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("{context} {}", path.display()))
        })
        .transpose()
}

fn drain_reader<R: Read>(reader: R, sender: std::sync::mpsc::SyncSender<io::Result<String>>) {
    for line in BufReader::new(reader).lines() {
        let failed = line.is_err();
        if sender.send(line).is_err() || failed {
            break;
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
    let mut read_error = None;
    // Drain both pipes concurrently: a full stderr pipe must not block stdout's EOF.
    std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(128);
        if let Some(stdout) = child.stdout.take() {
            let sender = sender.clone();
            scope.spawn(move || drain_reader(stdout, sender));
        }
        if let Some(stderr) = child.stderr.take() {
            let sender = sender.clone();
            scope.spawn(move || drain_reader(stderr, sender));
        }
        drop(sender);
        for line in receiver {
            match line {
                Ok(line) => {
                    ingest_stats_line(&mut stats, &line);
                    line_handler(&line);
                }
                Err(error) => remember_log_write_error(Err(error), &mut read_error),
            }
        }
    });
    let status = child.wait().context("wait for child")?;
    stats.exit_code = delegated_exit_code(status, "child", "stream_child_output_collecting");
    finish_log_writes(read_error, "read child output")?;
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
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1B' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1B' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) | None => {}
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
    let mut line = line.trim_end_matches('\r').to_string();
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

/// Linux reports `EIO` from the PTY master after its slave has closed.
/// This is the terminal equivalent of EOF, not a failed child process read.
#[cfg(unix)]
fn pty_read_error_is_end_of_stream(error: &std::io::Error) -> bool {
    cfg!(target_os = "linux") && error.raw_os_error() == Some(libc::EIO)
}

#[cfg(unix)]
fn stream_process_with_pty_unix<F, H>(
    cmd: &[String],
    log_path: Option<&Path>,
    env_overrides: &[(&str, Option<&str>)],
    mut line_handler: F,
    mut heartbeat_cb: H,
) -> Result<ProcessorStats>
where
    F: FnMut(&str),
    H: FnMut(),
{
    use std::io;
    use std::os::unix::io::FromRawFd;

    let mut log_file = open_stream_log(log_path, "open PTY stream log")?;
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

    let mut command = Command::new(&cmd[0]);
    apply_env_overrides(&mut command, env_overrides);
    let mut child = command
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
    let mut log_write_error = None;
    let mut emit_line = |line: &str| {
        if let Some(file) = log_file.as_mut() {
            remember_log_write_error(writeln!(file, "{line}"), &mut log_write_error);
        }
        line_handler(line);
    };

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
                let text = String::from_utf8_lossy(chunk);
                output_tail.push_str(&text);
                if output_tail.len() > 50_000 {
                    let mut start = output_tail.len() - 50_000;
                    while !output_tail.is_char_boundary(start) {
                        start += 1;
                    }
                    output_tail = output_tail[start..].to_string();
                }
                push_chunk_log_lines(&mut log_buffer, &text, &mut stats, &mut emit_line);
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
            Err(err) if pty_read_error_is_end_of_stream(&err) => break,
            Err(err) if matches!(err.kind(), io::ErrorKind::Interrupted) => {}
            Err(err) => {
                if child
                    .try_wait()
                    .context("check pty child after read error")?
                    .is_none()
                    && let Err(kill_error) = child.kill()
                    && child
                        .try_wait()
                        .context("check pty child after failed termination")?
                        .is_none()
                {
                    return Err(kill_error).context("terminate child after PTY read error");
                }
                child.wait().context("reap child after PTY read error")?;
                return Err(err).context("read pty child output");
            }
        }
    }

    if !log_buffer.trim().is_empty() {
        let final_line = log_buffer.clone();
        push_line(&mut log_buffer, &final_line, &mut stats, &mut emit_line);
    }
    for line in output_tail.lines() {
        ingest_stats_line(&mut stats, &strip_ansi_escapes(line));
    }

    let status = child.wait().context("wait for pty child")?;
    stats.exit_code = delegated_exit_code(status, &cmd[0], "stream_process_with_pty");
    finish_log_writes(log_write_error, "write PTY child output log")?;
    Ok(stats)
}

#[cfg(not(unix))]
fn stream_process_with_pty_unix<F, H>(
    cmd: &[String],
    _log_path: Option<&Path>,
    _env_overrides: &[(&str, Option<&str>)],
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
    stream_process_with_pty_with_env(cmd, log_path, &[], line_handler, heartbeat_cb)
}

fn apply_env_overrides(command: &mut Command, env_overrides: &[(&str, Option<&str>)]) {
    for (name, value) in env_overrides {
        if let Some(value) = value {
            command.env(name, value);
        } else {
            command.env_remove(name);
        }
    }
}

/// Stream a child with environment changes confined to that child process.
pub fn stream_process_with_pty_with_env<F, H>(
    cmd: &[String],
    log_path: Option<&Path>,
    env_overrides: &[(&str, Option<&str>)],
    mut line_handler: F,
    heartbeat_cb: H,
) -> Result<ProcessorStats>
where
    F: FnMut(&str),
    H: FnMut(),
{
    if pty_available() {
        return stream_process_with_pty_unix(
            cmd,
            log_path,
            env_overrides,
            line_handler,
            heartbeat_cb,
        );
    }
    let mut log_file = open_stream_log(log_path, "open process stream log")?;
    let mut command = Command::new(&cmd[0]);
    apply_env_overrides(&mut command, env_overrides);
    let child = command
        .args(&cmd[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn process for streaming")?;
    let mut log_write_error = None;
    let stats = stream_child_output_collecting(child, |line| {
        if let Some(file) = log_file.as_mut() {
            remember_log_write_error(writeln!(file, "{line}"), &mut log_write_error);
        }
        line_handler(line);
    })?;
    finish_log_writes(log_write_error, "write child output log")?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stats_extracts_numbers() {
        let output = "Succeeded: 10\nSkipped: 2\nIgnored: 3\nFailed: 1\nUnprocessed: 4";
        let stats = parse_stats_from_output(output);
        assert_eq!(stats.succeeded, 10);
        assert_eq!(stats.skipped, 2);
        assert_eq!(stats.ignored, 3);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.reported, [true; 4]);
        assert_eq!(stats.unprocessed, Some(4));
    }

    #[test]
    fn report_counters_keep_missing_values_unknown_and_skip_distinct() {
        let stats = parse_stats_from_output(
            "\x1b[32m│ ✅ Succeeded: 7 │\x1b[0m\n| [X] Failed: 0 |\n│ ⏭️ Skipped: 2 │\nfilename Failed: 99",
        );
        assert_eq!((stats.succeeded, stats.skipped, stats.failed), (7, 2, 0));
        assert_eq!(stats.reported, [true, true, false, true]);
        let malformed = parse_stats_from_output("Failed: 0\nFailed: 8.jpg");
        assert!(!malformed.reported[3]);
        let malformed_shape = parse_stats_from_output("Succeeded: 7\nSucceeded: 8 trailing");
        assert!(!malformed_shape.reported[0]);
        let malformed_message = parse_stats_from_output("Failed: 0\nFailed: 4 invalid message");
        assert!(!malformed_message.reported[3]);
        let empty = parse_stats_from_output("[ENCODE] no summary available");
        assert_eq!(empty.reported, [false; 4]);
        assert_eq!(empty.unprocessed, None);
    }

    #[test]
    fn converted_byte_receipts_are_exact_idempotent_and_fail_closed() {
        let receipt = r#"MFB_CONVERTED_BYTES={"schema_version":1,"scope":"converted_this_run","input_bytes":9007199254740993,"output_bytes":null}"#;
        let mut stats = parse_stats_from_output(receipt);
        let expected = ProcessorByteTotals {
            input_bytes: Some(9_007_199_254_740_993),
            output_bytes: None,
        };
        assert_eq!(stats.converted_bytes, Some(expected));
        ingest_stats_line(&mut stats, receipt);
        assert_eq!(stats.converted_bytes, Some(expected));
        assert!(!stats.converted_bytes_invalid);
        assert!(
            parse_stats_from_output(&format!("filename {receipt}"))
                .converted_bytes
                .is_none()
        );
        for invalid in [
            r#"{"schema_version":2,"scope":"converted_this_run","input_bytes":1,"output_bytes":2}"#,
            r#"{"schema_version":1,"scope":"all_files","input_bytes":1,"output_bytes":2}"#,
            r#"{"schema_version":1,"scope":"converted_this_run","input_bytes":-1,"output_bytes":2}"#,
            r#"{"schema_version":1,"scope":"converted_this_run","input_bytes":1.5,"output_bytes":2}"#,
            r#"{"schema_version":1,"scope":"converted_this_run","input_bytes":1}"#,
            r#"{"schema_version":1,"scope":"converted_this_run","input_bytes":1,"output_bytes":2}"#,
        ] {
            let mut bad = stats.clone();
            ingest_stats_line(&mut bad, &format!("MFB_CONVERTED_BYTES={invalid}"));
            ingest_stats_line(&mut bad, receipt);
            assert!(bad.converted_bytes_invalid);
            assert!(bad.converted_bytes.is_none());
        }
        let unknown = parse_stats_from_output(
            r#"MFB_CONVERTED_BYTES={"schema_version":1,"scope":"converted_this_run","input_bytes":null,"output_bytes":null}"#,
        );
        assert_eq!(
            unknown.converted_bytes,
            Some(ProcessorByteTotals::unknown())
        );
        assert!(!unknown.converted_bytes_invalid);
    }

    #[test]
    fn report_counters_accept_only_known_progress_overlays() {
        let valid =
            parse_stats_from_output("│ ✅ Succeeded: 7 │ │ 📊 X:0✓ I:0✓ P:0✓\nUnprocessed: 2");
        assert!(valid.reported[0]);
        assert_eq!(valid.succeeded, 7);
        assert_eq!(valid.unprocessed, Some(2));

        let invalid = parse_stats_from_output("Succeeded: 7 │ │ 📊 X:0✓ I:0✓ P:0✓ filename.jpg");
        assert!(!invalid.reported[0]);

        let video = parse_stats_from_output("│ ❌ Failed: 1 │ │ X:0✓ V:0✓1x");
        assert!(video.reported[3]);
        assert_eq!(video.failed, 1);
    }

    #[test]
    fn bare_malformed_final_count_labels_invalidate_prior_values() {
        let stats =
            parse_stats_from_output("Succeeded: 7\nSucceeded:\nUnprocessed: 2\nUnprocessed:");
        assert!(!stats.reported[0]);
        assert_eq!(stats.unprocessed, None);
        assert!(stats.unprocessed_invalid);
    }

    #[test]
    fn malformed_final_count_does_not_reuse_earlier_value() {
        let stats = parse_stats_from_output(
            "Succeeded: 7\nSucceeded: 18446744073709551616\nUnprocessed: 2\nUnprocessed: -1",
        );
        assert!(!stats.reported[0]);
        assert_eq!(stats.unprocessed, None);
        assert!(stats.unprocessed_invalid);
        assert!(!stats.counts_complete());
    }

    #[test]
    fn pending_parse_failure_is_not_a_legacy_missing_count() {
        let summary = "Succeeded: 1\nSkipped: 0\nIgnored: 0\nFailed: 0";
        assert!(parse_stats_from_output(summary).counts_complete());
        let invalid = parse_stats_from_output(&format!("{summary}\nUnprocessed: bad"));
        assert!(!invalid.counts_complete());
        let valid = parse_stats_from_output(&format!("{summary}\nUnprocessed: 2"));
        assert!(valid.counts_complete());
        assert_eq!(valid.unprocessed, Some(2));
    }

    #[cfg(unix)]
    #[test]
    fn child_environment_overrides_do_not_leak_to_parent() -> Result<()> {
        let key = "MFB_TEST_SCOPED_STREAM_ENV";
        let original = std::env::var_os(key);
        let command = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "printf 'Succeeded: %s\\n' \"$MFB_TEST_SCOPED_STREAM_ENV\"".to_owned(),
        ];
        let stats =
            stream_process_with_pty_with_env(&command, None, &[(key, Some("7"))], |_| {}, || {})?;
        assert_eq!(stats.succeeded, 7);
        assert!(stats.reported[0]);
        assert_eq!(stats.exit_code, 0);
        assert_eq!(std::env::var_os(key), original);
        Ok(())
    }

    #[test]
    fn test_push_line_strips_terminal_title_and_color_sequences_before_stats() {
        let mut stats = ProcessorStats::default();
        let mut buffer = String::new();
        push_line(
            &mut buffer,
            "\x1b]0;00s\x07\x1b[33mSkipped: 2\x1b[0m",
            &mut stats,
            &mut |_| {},
        );
        assert_eq!(stats.skipped, 2);
    }

    #[test]
    fn test_push_line_keeps_pty_crlf_content() {
        let mut stats = ProcessorStats::default();
        let mut buffer = String::new();
        let mut handled = Vec::new();

        push_line(&mut buffer, "first\r", &mut stats, &mut |line| {
            handled.push(line.to_string());
        });

        assert_eq!(handled, vec!["first".to_string()]);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn test_pty_available_on_unix() {
        assert!(pty_available());
    }

    #[cfg(unix)]
    #[test]
    fn test_pty_eio_end_of_stream_is_linux_specific() {
        let eio = std::io::Error::from_raw_os_error(libc::EIO);
        assert_eq!(
            pty_read_error_is_end_of_stream(&eio),
            cfg!(target_os = "linux")
        );
        assert!(!pty_read_error_is_end_of_stream(
            &std::io::Error::from_raw_os_error(libc::EBADF)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_pty_stream_tees_output_to_log_and_handler() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let log_path = temp.path().join("child.log");
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf 'first\\nsecond\\n'".to_string(),
        ];
        let mut handled = Vec::new();

        let stats = stream_process_with_pty(
            &command,
            Some(&log_path),
            |line| {
                handled.push(line.to_string());
            },
            || {},
        )?;

        assert_eq!(stats.exit_code, 0);
        assert_eq!(handled, vec!["first".to_string(), "second".to_string()]);
        let log = std::fs::read_to_string(log_path)?;
        assert!(log.contains("first"));
        assert!(log.contains("second"));
        Ok(())
    }

    #[test]
    fn child_log_write_failures_are_retained_and_propagated() {
        struct FailingWriter;

        impl Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "test log failure",
                ))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut first_error = None;
        let mut writer = FailingWriter;
        remember_log_write_error(writeln!(&mut writer, "child output"), &mut first_error);
        let error = finish_log_writes(first_error, "write child output log")
            .expect_err("child log write error must fail the stream result");
        assert!(format!("{error:#}").contains("write child output log"));
        assert!(format!("{error:#}").contains("test log failure"));
    }

    #[cfg(unix)]
    #[test]
    fn invalid_log_path_prevents_child_execution() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sentinel = temp.path().join("started");
        let command = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "touch \"$1\"".to_owned(),
            "sh".to_owned(),
            sentinel.to_string_lossy().into_owned(),
        ];
        let result = stream_process_with_pty(
            &command,
            Some(&temp.path().join("missing/child.log")),
            |_| {},
            || {},
        );
        assert!(result.is_err());
        assert!(!sentinel.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn pipes_are_drained_concurrently_without_losing_summary() -> Result<()> {
        let child = Command::new("sh")
            .args(["-c", "i=0; while [ \"$i\" -lt 12000 ]; do printf 'diagnostic padding padding padding\\n' >&2; i=$((i+1)); done; printf 'Succeeded: 1\\n'"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = i32::try_from(child.id())?;
        let (done, completion) = std::sync::mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if completion.recv_timeout(Duration::from_secs(10)).is_err() {
                // The test owns this still-unreaped child; bound a regression's pipe deadlock.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        });
        let mut diagnostics = 0;
        let result = stream_child_output_collecting(child, |line| {
            if line.starts_with("diagnostic") {
                diagnostics += 1;
            }
        });
        let _ = done.send(());
        watchdog.join().expect("watchdog must join");
        let stats = result?;
        assert_eq!(stats.exit_code, 0);
        assert_eq!(diagnostics, 12000);
        assert!(stats.reported[0]);
        assert_eq!(stats.succeeded, 1);
        Ok(())
    }
}

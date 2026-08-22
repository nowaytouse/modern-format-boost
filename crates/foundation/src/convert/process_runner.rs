//! Unified Process Runner with Deadlock Prevention and Loud Error Handling
//!
//! Provides a robust execution environment for all external tools (magick,
//! exiftool, cjxl, etc.) by ensuring stdout/stderr are concurrently consumed to
//! prevent kernel buffer deadlocks.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MANAGED_PROCESS_CAPTURE_MAX_BYTES: usize = 8 * 1024 * 1024;

fn capture_text_stream_bounded<R: Read>(
    stream: R,
    max_bytes: usize,
    stream_name: &'static str,
    command_line: &str,
    stream_verbose_stderr: bool,
) -> Result<String> {
    let mut reader = BufReader::new(stream);
    let mut output = String::with_capacity(max_bytes.min(8 * 1024));
    let mut line = Vec::new();
    let mut captured_bytes = 0usize;
    let mut exceeded = false;
    let mut capture_error = None;

    {
        let limit = u64::try_from(max_bytes.saturating_add(1)).unwrap_or(u64::MAX);
        let mut limited = (&mut reader).take(limit);

        loop {
            line.clear();
            let read = match limited.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) => {
                    capture_error = Some(anyhow::anyhow!(
                        "Failed to read child {stream_name}: {error}"
                    ));
                    break;
                }
            };
            captured_bytes = captured_bytes.saturating_add(read);
            if captured_bytes > max_bytes {
                exceeded = true;
                continue;
            }

            let had_newline = line.ends_with(b"\n");
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = if had_newline {
                line.strip_suffix(b"\r").unwrap_or(line)
            } else {
                line
            };
            match std::str::from_utf8(line) {
                Ok(line) => {
                    if stream_verbose_stderr
                        && crate::progress_mode::is_verbose_mode()
                        && should_stream_verbose_stderr_line(command_line, line)
                    {
                        crate::progress_mode::emit_stderr(&format!("   [stderr] {line}"));
                    }
                    output.push_str(line);
                    output.push('\n');
                }
                Err(error) => {
                    capture_error.get_or_insert_with(|| {
                        anyhow::anyhow!("Child {stream_name} was not valid UTF-8: {error}")
                    });
                }
            }
        }
    }

    let drain_error = std::io::copy(&mut reader, &mut std::io::sink()).err();

    if exceeded {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "process_output_limit",
            format!(
                "{stream_name} exceeded {max_bytes} captured bytes; remainder drained: {command_line}"
            ),
        );
        anyhow::bail!(
            "Child {stream_name} exceeded {max_bytes} captured bytes; remainder was drained"
        );
    }
    if let Some(error) = capture_error {
        return Err(error);
    }
    if let Some(error) = drain_error {
        anyhow::bail!("Failed to drain child {stream_name}: {error}");
    }

    Ok(output)
}

#[must_use]
pub const fn image_process_hard_timeout() -> Duration {
    Duration::from_secs(crate::constants::IMAGE_PROCESS_HARD_TIMEOUT_SECS)
}

#[must_use]
pub const fn animated_image_process_hard_timeout() -> Duration {
    Duration::from_secs(crate::constants::ANIMATED_IMAGE_PROCESS_HARD_TIMEOUT_SECS)
}

#[must_use]
pub const fn video_process_hard_timeout() -> Duration {
    Duration::from_secs(crate::constants::VIDEO_PROCESS_HARD_TIMEOUT_SECS)
}

/// Wait for a raw child process with a hard timeout.
///
/// # Errors
/// Returns an error if polling, killing, or reaping the child fails, or when
/// the timeout expires.
pub fn wait_child_with_timeout(
    child: &mut Child,
    timeout: Duration,
    context: &str,
) -> Result<ExitStatus> {
    wait_child_with_liveness_timeout(child, timeout, timeout, context)
}

/// Wait for a child process using a diagnostic soft deadline and a hard
/// deadline. A process that is still alive at the soft deadline is allowed to
/// continue; only the hard deadline kills it.
///
/// # Errors
/// Returns an error if polling, killing, or reaping the child fails or when the
/// hard deadline expires.
pub fn wait_child_with_liveness_timeout(
    child: &mut Child,
    soft_timeout: Duration,
    hard_timeout: Duration,
    context: &str,
) -> Result<ExitStatus> {
    let soft_timeout = soft_timeout.min(hard_timeout);
    let start = Instant::now();
    let mut soft_deadline_reported = soft_timeout == hard_timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("Failed to poll process for {context}"))?
        {
            return Ok(status);
        }
        let elapsed = start.elapsed();
        if !soft_deadline_reported && elapsed >= soft_timeout {
            soft_deadline_reported = true;
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "process_soft_timeout_alive",
                format!(
                    "{context}: subprocess is still alive after {soft_timeout:?}; allowing it to \
                     continue until hard timeout {hard_timeout:?}"
                ),
            );
        }
        if elapsed >= hard_timeout {
            kill_child_after_timeout(child, context)?;
            let status = child
                .wait()
                .with_context(|| format!("Failed to reap timed-out process for {context}"))?;
            anyhow::bail!(
                "{context} timed out at hard timeout after {elapsed:?} / {hard_timeout:?} \
                 (soft timeout {soft_timeout:?}; subprocess killed, \
                 termination={termination})",
                termination = crate::media_conversion_gate::process_termination_label_for_context(
                    &status,
                    "process_runner_wait_child_timeout",
                    context,
                ),
            );
        }
        thread::sleep(Duration::from_millis(35));
    }
}

/// Run a captured command with liveness-aware deadlines while preserving the
/// standard-library `Output` shape used by legacy callers.
///
/// # Errors
/// Returns an I/O error when spawning, waiting, draining, or the hard deadline
/// fails.
pub fn run_command_with_liveness_timeout(
    command: &mut Command,
    soft_timeout: Duration,
    hard_timeout: Duration,
    context: &str,
) -> std::io::Result<Output> {
    let output = ManagedProcess::spawn_captured(command)
        .and_then(|process| process.wait_liveness_timeout(soft_timeout, hard_timeout, context))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(Output {
        status: output.status,
        stdout: output.stdout.into_bytes(),
        stderr: output.stderr.into_bytes(),
    })
}

fn kill_child_after_timeout(child: &mut Child, context: &str) -> Result<Option<ExitStatus>> {
    match child.kill() {
        Ok(()) => Ok(None),
        Err(kill_err) => {
            if let Some(status) = child.try_wait().with_context(|| {
                format!("Failed to poll process after kill failure for {context}")
            })? {
                return Ok(Some(status));
            }
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "process_timeout_kill",
                format!("{context}: failed to kill timed-out subprocess: {kill_err}"),
            );
            anyhow::bail!("{context} timed out and subprocess kill failed: {kill_err}");
        }
    }
}

/// A running process with deadlock prevention.
pub struct ManagedProcess {
    child: Child,
    stdout_thread: Option<JoinHandle<Result<String>>>,
    stderr_thread: Option<JoinHandle<Result<String>>>,
    command_line: String,
    audit_nonzero_exit: bool,
}

impl ManagedProcess {
    /// Spawn a command and start monitoring its output.
    ///
    /// # Errors
    /// Returns error if spawning fails.
    pub fn spawn(cmd: &mut Command) -> Result<Self> {
        Self::spawn_with_policy(cmd, true, true, MANAGED_PROCESS_CAPTURE_MAX_BYTES)
    }

    /// Spawn a command whose non-zero exit is an expected, caller-inspected
    /// outcome. Output remains captured, but is neither streamed nor escalated
    /// before the caller has classified it.
    ///
    /// # Errors
    /// Returns an error if spawning fails.
    pub fn spawn_captured(cmd: &mut Command) -> Result<Self> {
        Self::spawn_with_policy(cmd, false, false, MANAGED_PROCESS_CAPTURE_MAX_BYTES)
    }

    fn spawn_with_policy(
        cmd: &mut Command,
        stream_verbose_stderr: bool,
        audit_nonzero_exit: bool,
        capture_max_bytes: usize,
    ) -> Result<Self> {
        let command_line = crate::common_utils::format_command_for_audit(cmd);
        crate::log_debug!(
            &crate::infra::static_logs::messages::MSG_PROCESS_SPAWN.replace("{}", &command_line)
        );

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn command: {command_line}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

        let stdout_command_line = command_line.clone();
        let stdout_thread = thread::spawn(move || {
            capture_text_stream_bounded(
                stdout,
                capture_max_bytes,
                "stdout",
                &stdout_command_line,
                false,
            )
        });

        let stderr_command_line = command_line.clone();
        let stderr_thread = thread::spawn(move || {
            capture_text_stream_bounded(
                stderr,
                capture_max_bytes,
                "stderr",
                &stderr_command_line,
                stream_verbose_stderr,
            )
        });

        Ok(Self {
            child,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            command_line,
            audit_nonzero_exit,
        })
    }

    /// Get the process ID (PID) of the managed process.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Kill the managed process using platform-specific tools.
    /// Uses `taskkill` on Windows, `kill` on Unix.
    ///
    /// # Errors
    /// Returns error if the kill command fails to execute or the process cannot
    /// be terminated.
    pub fn kill(&self) -> anyhow::Result<()> {
        let pid = self.pid();

        #[cfg(target_os = "windows")]
        {
            use crate::builder_base::ToolBuilder;
            let status = crate::tool_builders::TaskkillBuilder::new()
                .pid(pid)
                .force(true)
                .build()
                .status()
                .map_err(|e| anyhow::anyhow!("Failed to run taskkill for process {pid}: {e}"))?;
            external_kill_status_result(pid, status)
        }

        #[cfg(not(target_os = "windows"))]
        {
            use crate::builder_base::ToolBuilder;
            let status = crate::tool_builders::KillBuilder::new()
                .signal("-9")
                .pid(pid)
                .build()
                .status()
                .map_err(|e| anyhow::anyhow!("Failed to run kill for process {pid}: {e}"))?;
            external_kill_status_result(pid, status)
        }
    }

    fn finalize(mut self, status: ExitStatus) -> Result<ProcessOutput> {
        let stdout_thread = self
            .stdout_thread
            .take()
            .ok_or_else(|| anyhow::anyhow!("Stdout thread missing during join"))?;
        let stderr_thread = self
            .stderr_thread
            .take()
            .ok_or_else(|| anyhow::anyhow!("Stderr thread missing during join"))?;
        let stdout = stdout_thread.join();
        let stderr = stderr_thread.join();
        let stdout =
            stdout.map_err(|error| anyhow::anyhow!("Stdout reader panicked: {error:?}"))??;
        let stderr =
            stderr.map_err(|error| anyhow::anyhow!("Stderr reader panicked: {error:?}"))??;

        if !status.success() && self.audit_nonzero_exit {
            crate::media_conversion_gate::delivery_tool_process_failed_audit(
                "process_runner",
                &self.command_line,
                &status,
            );
        }

        Ok(ProcessOutput {
            status,
            stdout,
            stderr,
            command_line: self.command_line,
        })
    }

    /// Wait for the process to complete and return its status and outputs.
    ///
    /// # Errors
    /// Returns error if waiting fails or output threads panic.
    pub fn wait(self) -> Result<ProcessOutput> {
        let mut this = self;
        let status = this
            .child
            .wait()
            .with_context(|| format!("Failed to wait for process: {}", this.command_line))?;
        this.finalize(status)
    }

    /// Wait for completion with a hard timeout. On timeout the child is killed,
    /// outputs are drained, and an error is returned with captured tail
    /// context.
    ///
    /// # Errors
    /// Returns error if waiting, killing, or draining the process fails, or
    /// when the deadline is exceeded.
    pub fn wait_timeout(self, timeout: Duration, context: &str) -> Result<ProcessOutput> {
        self.wait_liveness_timeout(timeout, timeout, context)
    }

    /// Wait with a diagnostic soft deadline and a hard kill deadline.
    ///
    /// # Errors
    /// Returns error if waiting, killing, or draining fails, when the hard
    /// deadline is exceeded.
    pub fn wait_liveness_timeout(
        self,
        soft_timeout: Duration,
        hard_timeout: Duration,
        context: &str,
    ) -> Result<ProcessOutput> {
        let soft_timeout = soft_timeout.min(hard_timeout);
        let mut this = self;
        let start = Instant::now();
        let mut soft_deadline_reported = soft_timeout == hard_timeout;
        loop {
            if let Some(status) = this.child.try_wait().with_context(|| {
                format!(
                    "Failed to poll process: {command_line}",
                    command_line = this.command_line
                )
            })? {
                return this.finalize(status);
            }
            let elapsed = start.elapsed();
            if !soft_deadline_reported && elapsed >= soft_timeout {
                soft_deadline_reported = true;
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "process_soft_timeout_alive",
                    format!(
                        "{context}: subprocess is still alive after {soft_timeout:?}; allowing it \
                         to continue until hard timeout {hard_timeout:?}"
                    ),
                );
            }
            if elapsed >= hard_timeout {
                let child_id = this.child.id();
                let command_line = this.command_line.clone();
                kill_child_after_timeout(&mut this.child, &command_line)?;
                let status = this
                    .child
                    .wait()
                    .with_context(|| format!("Failed to reap timed-out process: {command_line}"))?;
                let output = this.finalize(status)?;
                let elapsed = start.elapsed();
                let stdout_len = output.stdout.len();
                let stderr_len = output.stderr.len();
                let stdout_tail =
                    crate::media_conversion_gate::delivery_subprocess_log_tail_or_empty(
                        output
                            .stdout
                            .lines()
                            .rev()
                            .find(|line| !line.trim().is_empty()),
                    );

                let termination_label =
                    crate::media_conversion_gate::process_termination_label_for_context(
                        &output.status,
                        "process_runner_wait_timeout",
                        &command_line,
                    );
                let stderr_summary = if stderr_len <= 2000 {
                    output.stderr.trim().to_string()
                } else {
                    let tail = output
                        .stderr
                        .lines()
                        .rev()
                        .take(20)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("... [truncated {stderr_len} bytes] ...\n{tail}")
                };
                anyhow::bail!(
                    "{context} timed out at hard timeout after {elapsed:?} / {hard_timeout:?} \
                     (soft timeout {soft_timeout:?}; subprocess killed, \
                     termination={termination_label})\n   Command: {command_line}\n   Pid: \
                     {child_id}\n   Stdout bytes: {stdout_len}, stderr bytes: {stderr_len}\n   \
                     Stdout tail: {stdout_tail}\n   Stderr:\n{stderr_summary}\n",
                );
            }
            thread::sleep(Duration::from_millis(35));
        }
    }
}

fn external_kill_status_result(pid: u32, status: ExitStatus) -> anyhow::Result<()> {
    if status.success() {
        return Ok(());
    }

    let termination = crate::media_conversion_gate::process_termination_label_for_context(
        &status,
        "process_runner_kill",
        format!("pid={pid}"),
    );
    crate::media_conversion_gate::delivery_runtime_batch_audit(
        "process_kill_status",
        format!("external kill command for pid {pid} failed with {termination}"),
    );
    anyhow::bail!("Failed to kill process {pid}: kill command ended with {termination}");
}

fn should_stream_verbose_stderr_line(command_line: &str, line: &str) -> bool {
    let trimmed = line.trim();
    !(command_line.contains("osxphotos")
        && trimmed.starts_with("Using last opened Photos library:"))
}

pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub command_line: String,
}

impl ProcessOutput {
    /// Ensures the process succeeded, or returns a "Loud" error.
    ///
    /// # Errors
    /// Returns error with detailed diagnostics if the process failed.
    pub fn check_loud(self, context: &str) -> Result<Self> {
        if self.status.success() {
            Ok(self)
        } else {
            let err_msg = format!(
                "{} failed ({})\n   Command: {}\n   Error: {}",
                context,
                crate::media_conversion_gate::process_termination_label_for_context(
                    &self.status,
                    "process_runner",
                    &self.command_line,
                ),
                self.command_line,
                crate::media_conversion_gate::tool_stderr_last_line_label(&self.stderr, context,)
            );
            crate::log_failure!(crate::infra::static_logs::messages::LABEL_TOOLS, &err_msg);
            Err(anyhow::anyhow!(err_msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_managed_process_success() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello world");

        let process = ManagedProcess::spawn(&mut cmd).unwrap_or_else(|e| {
            unreachable!(
                "CRITICAL: ManagedProcess spawn failed in test (error: {:?})",
                e
            )
        });
        let output = process.wait().unwrap_or_else(|e| {
            unreachable!(
                "CRITICAL: ManagedProcess wait failed in test (error: {:?})",
                e
            )
        });

        assert!(output.status.success());
        assert_eq!(output.stdout.trim(), "hello world");
    }

    #[test]
    fn test_managed_process_failure() {
        // Use a command that is likely to fail
        let mut cmd = Command::new("ls");
        cmd.arg("/non_existent_directory_gemini_test");

        let process = ManagedProcess::spawn(&mut cmd).unwrap_or_else(|e| {
            unreachable!(
                "CRITICAL: ManagedProcess spawn failed in test (error: {:?})",
                e
            )
        });
        let output = process.wait().unwrap_or_else(|e| {
            unreachable!(
                "CRITICAL: ManagedProcess wait failed in test (error: {:?})",
                e
            )
        });

        assert!(!output.status.success());
        assert_ne!(output.stderr.len(), 0);
    }

    #[test]
    fn osxphotos_library_notice_is_captured_but_not_streamed_as_verbose_stderr() {
        assert!(!should_stream_verbose_stderr_line(
            "/Users/example/.local/bin/osxphotos query --json",
            "Using last opened Photos library: /Users/example/Pictures/Main.photoslibrary",
        ));
        assert!(should_stream_verbose_stderr_line(
            "/Users/example/.local/bin/osxphotos query --json",
            "Error: query failed",
        ));
        assert!(should_stream_verbose_stderr_line(
            "/usr/bin/osascript -e <inline-script bytes=12 lines=1>",
            "Using last opened Photos library: /Users/example/Pictures/Main.photoslibrary",
        ));
    }

    #[cfg(unix)]
    #[test]
    fn wait_child_with_timeout_kills_slow_process() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 2")
            .spawn()
            .unwrap_or_else(|e| panic!("spawn sleep fixture: {e:?}"));

        let err = wait_child_with_timeout(
            &mut child,
            Duration::from_millis(10),
            "unit timeout fixture",
        )
        .unwrap_err();

        assert!(err.to_string().contains("timed out"));
    }

    #[cfg(unix)]
    #[test]
    fn liveness_timeout_allows_live_process_past_soft_deadline() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 0.08")
            .spawn()
            .unwrap_or_else(|e| panic!("spawn sleep fixture: {e:?}"));

        let status = wait_child_with_liveness_timeout(
            &mut child,
            Duration::from_millis(10),
            Duration::from_secs(1),
            "unit liveness fixture",
        )
        .unwrap_or_else(|e| panic!("live process should finish before hard deadline: {e:?}"));

        assert!(status.success());
    }

    #[cfg(unix)]
    #[test]
    fn liveness_timeout_kills_process_at_hard_deadline() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 2");
        let result = ManagedProcess::spawn_captured(&mut command).and_then(|process| {
            process.wait_liveness_timeout(
                Duration::from_millis(10),
                Duration::from_millis(40),
                "unit hard deadline fixture",
            )
        });
        let err = match result {
            Ok(_) => panic!("managed process must fail at the hard deadline"),
            Err(err) => err,
        };

        let message = err.to_string();
        assert!(message.contains("hard timeout"), "{message}");
        assert!(message.contains("termination=signal("), "{message}");
        assert!(!message.contains("exit_code=-1"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn managed_liveness_timeout_allows_live_process_past_soft_deadline() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 0.08; printf done");
        let output = ManagedProcess::spawn_captured(&mut command)
            .and_then(|process| {
                process.wait_liveness_timeout(
                    Duration::from_millis(10),
                    Duration::from_secs(1),
                    "managed liveness fixture",
                )
            })
            .unwrap_or_else(|e| panic!("managed live process should finish: {e:?}"));

        assert!(output.status.success());
        assert_eq!(output.stdout, "done\n");
    }

    #[cfg(unix)]
    #[test]
    fn captured_output_limit_drains_noisy_process_before_failing() {
        let mut command = Command::new("sh");
        command.arg("-c").arg(
            "i=0; while [ \"$i\" -lt 8192 ]; do printf 0123456789abcdef; i=$((i+1)); done; printf done >&2",
        );
        let result =
            ManagedProcess::spawn_with_policy(&mut command, false, false, 64).and_then(|process| {
                process.wait_timeout(Duration::from_secs(2), "bounded output fixture")
            });
        let error = match result {
            Ok(_) => panic!("oversized captured output must fail loudly"),
            Err(error) => error,
        };
        let message = error.to_string();

        assert!(message.contains("stdout"), "{message}");
        assert!(message.contains("exceeded 64 captured bytes"), "{message}");
    }

    #[test]
    fn capture_text_stream_bounded_scopes_borrow_and_drains() {
        let data = b"line1\nline2\nline3\nline4\n";
        let captured =
            capture_text_stream_bounded(&data[..], 12, "test_stream", "mock_command", false);
        let err = captured.unwrap_err();
        assert!(err.to_string().contains("exceeded 12 captured bytes"));
    }

    #[test]
    fn media_hard_deadlines_match_design() {
        assert_eq!(image_process_hard_timeout(), Duration::from_hours(168));
        assert_eq!(
            animated_image_process_hard_timeout(),
            Duration::from_hours(168)
        );
        assert_eq!(video_process_hard_timeout(), Duration::from_hours(336));
    }

    #[cfg(unix)]
    #[test]
    fn external_kill_status_failure_is_loud() {
        let status = Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .status()
            .unwrap_or_else(|e| panic!("spawn exit fixture: {e:?}"));

        let err = external_kill_status_result(123_456, status).unwrap_err();

        assert!(err.to_string().contains("exit"));
        assert!(err.to_string().contains('7'));
    }
}

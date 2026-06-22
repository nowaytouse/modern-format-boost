//! Unified Process Runner with Deadlock Prevention and Loud Error Handling
//!
//! Provides a robust execution environment for all external tools (magick, exiftool, cjxl, etc.)
//! by ensuring stdout/stderr are concurrently consumed to prevent kernel buffer deadlocks.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Wait for a raw child process with a hard timeout.
///
/// # Errors
/// Returns an error if polling, killing, or reaping the child fails, or when the
/// timeout expires.
pub fn wait_child_with_timeout(
    child: &mut Child,
    timeout: Duration,
    context: &str,
) -> Result<ExitStatus> {
    let start = Instant::now();
    let deadline = start + timeout;
    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("Failed to poll process for {context}"))?
        {
            return Ok(status);
        }
        if Instant::now() > deadline {
            kill_child_after_timeout(child, context)?;
            let status = child
                .wait()
                .with_context(|| format!("Failed to reap timed-out process for {context}"))?;
            anyhow::bail!(
                "{context} timed out after {elapsed:?} / {timeout:?} (subprocess killed, exit_code={exit_code})",
                elapsed = start.elapsed(),
                exit_code = crate::media_conversion_gate::process_exit_code_for_context(
                    status.code(),
                    "process_runner_wait_child_timeout",
                    context,
                ),
            );
        }
        thread::sleep(Duration::from_millis(35));
    }
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
}

impl ManagedProcess {
    /// Spawn a command and start monitoring its output.
    ///
    /// # Errors
    /// Returns error if spawning fails.
    ///
    pub fn spawn(cmd: &mut Command) -> Result<Self> {
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

        let stdout_thread = thread::spawn(move || {
            let mut buf = String::new();
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let line = line.map_err(|e| anyhow::anyhow!("Failed to read child stdout: {e}"))?;
                buf.push_str(&line);
                buf.push('\n');
            }
            Ok(buf)
        });

        let stderr_command_line = command_line.clone();
        let stderr_thread = thread::spawn(move || {
            let mut buf = String::new();
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let line = line.map_err(|e| anyhow::anyhow!("Failed to read child stderr: {e}"))?;
                if crate::progress_mode::is_verbose_mode()
                    && should_stream_verbose_stderr_line(&stderr_command_line, &line)
                {
                    crate::progress_mode::emit_stderr(&format!("   [stderr] {line}"));
                }
                buf.push_str(&line);
                buf.push('\n');
            }
            Ok(buf)
        });

        Ok(Self {
            child,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            command_line,
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
    /// Returns error if the kill command fails to execute or the process cannot be terminated.
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
        let stdout = self
            .stdout_thread
            .take()
            .ok_or_else(|| anyhow::anyhow!("Stdout thread missing during join"))?
            .join()
            .map_err(|e| anyhow::anyhow!("Stdout reader panicked: {e:?}"))??;
        let stderr = self
            .stderr_thread
            .take()
            .ok_or_else(|| anyhow::anyhow!("Stderr thread missing during join"))?
            .join()
            .map_err(|e| anyhow::anyhow!("Stderr reader panicked: {e:?}"))??;

        if !status.success() {
            crate::media_conversion_gate::delivery_tool_process_failed_audit(
                "process_runner",
                &self.command_line,
                status.code(),
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
    /// outputs are drained, and an error is returned with captured tail context.
    ///
    /// # Errors
    /// Returns error if waiting, killing, or draining the process fails, or when
    /// the deadline is exceeded.
    pub fn wait_timeout(self, timeout: Duration, context: &str) -> Result<ProcessOutput> {
        let mut this = self;
        let start = Instant::now();
        let deadline = start + timeout;
        loop {
            if let Some(status) = this.child.try_wait().with_context(|| {
                format!(
                    "Failed to poll process: {command_line}",
                    command_line = this.command_line
                )
            })? {
                return this.finalize(status);
            }
            if Instant::now() > deadline {
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

                let exit_code_label = crate::media_conversion_gate::process_exit_code_for_context(
                    output.status.code(),
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
                    "{context} timed out after {elapsed:?} / {timeout:?} (subprocess killed, exit_code={exit_code_label})\n   Command: {command_line}\n   Pid: {child_id}\n   Stdout bytes: {stdout_len}, stderr bytes: {stderr_len}\n   Stdout tail: {stdout_tail}\n   Stderr:\n{stderr_summary}\n",
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

    let code = crate::media_conversion_gate::process_exit_code_for_context(
        status.code(),
        "process_runner_kill",
        format!("pid={pid}"),
    );
    crate::media_conversion_gate::delivery_runtime_batch_audit(
        "process_kill_status",
        format!("external kill command for pid {pid} failed with exit code {code}"),
    );
    anyhow::bail!("Failed to kill process {pid}: kill command exited with code {code}");
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
                "{} failed (exit code: {})\n   Command: {}\n   Error: {}",
                context,
                crate::media_conversion_gate::process_exit_code_for_context(
                    self.status.code(),
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
        assert!(!output.stderr.is_empty());
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

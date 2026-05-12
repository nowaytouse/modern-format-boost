//! Unified Process Runner with Deadlock Prevention and Loud Error Handling
//!
//! Provides a robust execution environment for all external tools (magick, exiftool, cjxl, etc.)
//! by ensuring stdout/stderr are concurrently consumed to prevent kernel buffer deadlocks.

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};

/// A running process with deadlock prevention.
pub struct ManagedProcess {
    child: Child,
    stdout_thread: Option<JoinHandle<String>>,
    stderr_thread: Option<JoinHandle<String>>,
    command_line: String,
}

impl ManagedProcess {
    /// Spawn a command and start monitoring its output.
    ///
    /// # Errors
    /// Returns error if spawning fails.
    ///
    /// # Panics
    /// Panics if the process fails to capture stdout or stderr.
    pub fn spawn(cmd: &mut Command) -> Result<Self> {
        let command_line = format!("{cmd:?}");
        crate::log_debug!(&format!("Spawning managed process: {command_line}"));

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
            for line in reader.lines().map_while(Result::ok) {
                buf.push_str(&line);
                buf.push('\n');
            }
            buf
        });

        let stderr_thread = thread::spawn(move || {
            let mut buf = String::new();
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if crate::progress_mode::is_verbose_mode() {
                    crate::progress_mode::emit_stderr(&format!("   [stderr] {line}"));
                }
                buf.push_str(&line);
                buf.push('\n');
            }
            buf
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
            crate::tool_builders::TaskkillBuilder::new()
                .pid(pid)
                .force(true)
                .build()
                .status()
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("Failed to kill process {pid}: {e}"))
        }

        #[cfg(not(target_os = "windows"))]
        {
            use crate::builder_base::ToolBuilder;
            crate::tool_builders::KillBuilder::new()
                .signal("-9")
                .pid(pid)
                .build()
                .status()
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("Failed to kill process {pid}: {e}"))
        }
    }

    /// Wait for the process to complete and return its status and outputs.
    ///
    /// # Errors
    /// Returns error if waiting fails or output threads panic.
    ///
    /// # Panics
    /// Panics if the output threads are missing during join.
    pub fn wait(mut self) -> Result<ProcessOutput> {
        let status = self
            .child
            .wait()
            .with_context(|| format!("Failed to wait for process: {}", self.command_line))?;

        let stdout = self
            .stdout_thread
            .take()
            .ok_or_else(|| anyhow::anyhow!("Stdout thread missing during join"))?
            .join()
            .map_err(|_| anyhow::anyhow!("Stdout reader panicked"))?;
        let stderr = self
            .stderr_thread
            .take()
            .ok_or_else(|| anyhow::anyhow!("Stderr thread missing during join"))?
            .join()
            .map_err(|_| anyhow::anyhow!("Stderr reader panicked"))?;

        if !status.success() {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_FFMPEG,
                &format!(
                    "Process failed ({}): {}",
                    status.code().unwrap_or(-1),
                    self.command_line
                )
            );
        }

        Ok(ProcessOutput {
            status,
            stdout,
            stderr,
            command_line: self.command_line,
        })
    }
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
                self.status.code().unwrap_or(-1),
                self.command_line,
                self.stderr
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("No error output")
            );
            crate::log_failure!("Tool", &err_msg);
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

        let process = ManagedProcess::spawn(&mut cmd).unwrap();
        let output = process.wait().unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout.trim(), "hello world");
    }

    #[test]
    fn test_managed_process_failure() {
        // Use a command that is likely to fail
        let mut cmd = Command::new("ls");
        cmd.arg("/non_existent_directory_gemini_test");

        let process = ManagedProcess::spawn(&mut cmd).unwrap();
        let output = process.wait().unwrap();

        assert!(!output.status.success());
        assert!(!output.stderr.is_empty());
    }
}

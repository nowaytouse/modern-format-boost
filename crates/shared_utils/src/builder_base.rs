//! Core infrastructure for Tool Builders
//!
//! Provides common functionality for all media tool builders including:
//! - Standard input/output handling
//! - Argument collection
//! - Deadlock-safe execution via `ManagedProcess`
//! - Availability checking with caching

use crate::process_runner::{ManagedProcess, ProcessOutput};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::RwLock;

static TOOL_CACHE: std::sync::LazyLock<RwLock<HashMap<String, bool>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

pub trait ToolBuilder {
    fn get_command_name(&self) -> &str;
    fn build(&self) -> Command;

    /// Returns a `Command` initialized with the resolved path of the tool.
    /// Uses `resolve_tool_path` to handle macOS GUI app fallbacks.
    fn get_resolved_command(&self) -> Command {
        let name = self.get_command_name();
        let path = crate::common_utils::resolve_tool_path(name)
            .unwrap_or_else(|| std::path::PathBuf::from(name));
        Command::new(path)
    }

    /// Returns the arguments used for availability check (default: --version)
    fn get_check_args(&self) -> &[&str] {
        &["--version"]
    }

    /// Checks if the tool is available in the system.
    ///
    /// Positive results are cached. Negative results are NOT cached: under
    /// heavy parallel workloads a transient failure in `Command::output`
    /// (fd exhaustion, signal interruption, IO pressure) would otherwise
    /// latch the cache to `false` and every subsequent file would wrongly
    /// report the tool missing.
    fn check_available(&self) -> bool {
        let name = self.get_command_name();

        {
            let cache = TOOL_CACHE
                .read()
                .expect("TOOL_CACHE lock poisoned during read");
            if let Some(&available) = cache.get(name)
                && available
            {
                return true;
            }
        }

        // Fast path: if resolve_tool_path finds the binary on disk, it's
        // available. Skip the --version spawn that can fail transiently.
        if crate::common_utils::resolve_tool_path(name).is_some() {
            {
                let mut cache = TOOL_CACHE
                    .write()
                    .expect("TOOL_CACHE lock poisoned during write");
                cache.insert(name.to_string(), true);
            }
            return true;
        }

        // Slow path: try running the tool. Only cache on success.
        let path = std::path::PathBuf::from(name);
        let mut cmd = Command::new(&path);
        cmd.args(self.get_check_args());

        let available = cmd
            .output()
            .or_else(|_| Command::new(&path).arg("-version").output())
            .is_ok_and(|o| o.status.success());

        if available {
            let mut cache = TOOL_CACHE
                .write()
                .expect("TOOL_CACHE lock poisoned during write");
            cache.insert(name.to_string(), true);
        }
        available
    }

    /// Spawns the command and returns a managed process.
    ///
    /// # Errors
    /// Returns an error if the process fails to spawn.
    fn spawn_managed(&self) -> anyhow::Result<ManagedProcess> {
        let mut cmd = self.build();
        ManagedProcess::spawn(&mut cmd)
    }

    /// Executes the command and returns the output, handling deadlocks and errors.
    ///
    /// # Errors
    /// Returns an error if the process fails to spawn, wait, or if it exits with an error status.
    fn execute(&self, context: &str) -> anyhow::Result<ProcessOutput> {
        self.spawn_managed()?.wait()?.check_loud(context)
    }
}

/// Common components for builders to reduce boilerplate.
#[derive(Debug, Default, Clone)]
pub struct CommonArgs {
    pub inputs: Vec<PathBuf>,
    pub output: Option<PathBuf>,
    pub extra_args: Vec<String>,
}

impl CommonArgs {
    pub fn push_arg<S: AsRef<str>>(&mut self, arg: S) {
        self.extra_args.push(arg.as_ref().to_string());
    }

    pub fn push_args<I, S>(&mut self, args: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            self.extra_args.push(arg.as_ref().to_string());
        }
    }
}

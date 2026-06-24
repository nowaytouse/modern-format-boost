//! Core infrastructure for Tool Builders
//!
//! Provides common functionality for all media tool builders including:
//! - Standard input/output handling
//! - Argument collection
//! - Deadlock-safe execution via `ManagedProcess`
//! - Availability checking with caching

use crate::process_runner::{ManagedProcess, ProcessOutput};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
        let path = crate::common_utils::resolve_tool_path_or_audit(name);
        Command::new(path)
    }

    /// Returns the arguments used for availability check (default: --version)
    fn get_check_args(&self) -> &[&str] {
        &["--version"]
    }

    /// Checks if the tool is available in the system.
    fn check_available(&self) -> bool {
        let name = self.get_command_name();

        {
            let cache = crate::media_conversion_gate::rwlock_read_guard_or_recover(
                "tool_builder_cache",
                TOOL_CACHE.read(),
            );
            if let Some(&available) = cache.get(name)
                && available
            {
                return true;
            }
        }

        // Fast path: if resolve_tool_path finds the binary on disk, it's available.
        if crate::common_utils::resolve_tool_path(name).is_some() {
            {
                let mut cache = crate::media_conversion_gate::rwlock_write_guard_or_recover(
                    "tool_builder_cache",
                    TOOL_CACHE.write(),
                );
                cache.insert(name.to_string(), true);
            }
            return true;
        }

        // Slow path: try running the tool.
        let path = crate::common_utils::resolve_tool_path_or_audit(name);
        let mut cmd = Command::new(&path);
        cmd.args(self.get_check_args());

        let available = match cmd
            .output()
            .or_else(|_| Command::new(&path).arg("-version").output())
        {
            Ok(output) => output.status.success(),
            Err(e) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "tool_probe",
                    format!("tool builder availability probe failed for {name}: {e}"),
                );
                false
            }
        };

        if available {
            let mut cache = crate::media_conversion_gate::rwlock_write_guard_or_recover(
                "tool_builder_cache",
                TOOL_CACHE.write(),
            );
            cache.insert(name.to_string(), true);
        }
        available
    }

    /// Spawns the command and returns a managed process.
    ///
    /// # Errors
    /// Returns an error if the process fails to start.
    fn spawn_managed(&self) -> anyhow::Result<ManagedProcess> {
        let mut cmd = self.build();
        ManagedProcess::spawn(&mut cmd)
    }

    /// Executes the command and returns the output, handling deadlocks and
    /// errors.
    ///
    /// # Errors
    /// Returns an error if execution or waiting fails.
    fn execute(&self, context: &str) -> anyhow::Result<ProcessOutput> {
        self.spawn_managed()?.wait()?.check_loud(context)
    }
}

/// Common components for builders to reduce boilerplate.
#[derive(Debug, Default, Clone)]
pub struct BaseBuilder {
    pub inputs: Vec<PathBuf>,
    pub output: Option<PathBuf>,
    pub extra_args: Vec<String>,
}

impl BaseBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.inputs.push(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            self.extra_args.push(arg.as_ref().to_string());
        }
        self
    }

    pub fn apply_to_command(&self, cmd: &mut Command) {
        for arg in &self.extra_args {
            cmd.arg(arg);
        }
    }

    /// Safely applies the first input to the command, optionally with a
    /// preceding flag.
    pub fn apply_first_input(&self, cmd: &mut Command, flag: Option<&str>) {
        if let Some(input) = self.inputs.first() {
            if let Some(f) = flag {
                cmd.arg(f);
            }
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }
    }

    /// Safely applies all inputs to the command, optionally with a preceding
    /// flag for each.
    pub fn apply_all_inputs(&self, cmd: &mut Command, flag: Option<&str>) {
        for input in &self.inputs {
            if let Some(f) = flag {
                cmd.arg(f);
            }
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }
    }

    /// Safely applies the output to the command, optionally with a preceding
    /// flag.
    pub fn apply_output(&self, cmd: &mut Command, flag: Option<&str>) {
        if let Some(output) = &self.output {
            if let Some(f) = flag {
                cmd.arg(f);
            }
            cmd.arg(crate::safe_path_arg(output).as_ref());
        }
    }
}

/// Macro to implement standard builder accessors that delegate to
/// `BaseBuilder`.
#[macro_export]
macro_rules! impl_base_builder_accessors {
    ($name:ident) => {
        impl $name {
            pub fn input<P: AsRef<std::path::Path>>(&mut self, path: P) -> &mut Self {
                self.base.input(path);
                self
            }

            pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
                self.base.arg(arg);
                self
            }

            pub fn args<I, S>(&mut self, args: I) -> &mut Self
            where
                I: IntoIterator<Item = S>,
                S: AsRef<str>,
            {
                self.base.args(args);
                self
            }
        }
    };
}

/// Macro to implement standard builder accessors including `output`.
#[macro_export]
macro_rules! impl_base_builder_accessors_full {
    ($name:ident) => {
        $crate::impl_base_builder_accessors!($name);
        impl $name {
            pub fn output<P: AsRef<std::path::Path>>(&mut self, path: P) -> &mut Self {
                self.base.output(path);
                self
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base_builder_args() {
        let mut builder = BaseBuilder::new();
        builder.arg("-v").args(["-i", "input.mp4"]);
        assert_eq!(builder.extra_args, vec!["-v", "-i", "input.mp4"]);
    }

    #[test]
    fn test_base_builder_apply_to_command() {
        let mut builder = BaseBuilder::new();
        builder.arg("-v").arg("-y");
        let mut cmd = Command::new("ls");
        builder.apply_to_command(&mut cmd);
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args, vec!["-v", "-y"]);
    }

    #[test]
    fn test_base_builder_inputs_outputs() {
        let mut builder = BaseBuilder::new();
        builder.input("in1.mp4").input("in2.mp4").output("out.mp4");
        assert_eq!(builder.inputs.len(), 2);
        assert_eq!(builder.output, Some(PathBuf::from("out.mp4")));

        let mut cmd = Command::new("ffmpeg");
        builder.apply_first_input(&mut cmd, Some("-i"));
        builder.apply_output(&mut cmd, None);
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args, vec!["-i", "in1.mp4", "out.mp4"]);
    }
}

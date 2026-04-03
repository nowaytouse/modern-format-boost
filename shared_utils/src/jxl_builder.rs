//! Type-safe builders for JPEG XL (cjxl, djxl) tools.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use crate::constants;

/// Builder for constructing `cjxl` commands.
#[derive(Debug, Default)]
pub struct CjxlBuilder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    distance: Option<f32>,
    effort: Option<u8>,
    threads: Option<usize>,
    lossless_jpeg: bool,
    allow_jpeg_recon: Option<bool>,
    cicp: Option<String>,
    icc_profile: Option<PathBuf>,
    apple_compat: bool,
    extra_args: Vec<String>,
    use_stdin: bool,
    intensity_target: Option<f32>,
}

impl CjxlBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn distance(&mut self, distance: f32) -> &mut Self {
        self.distance = Some(distance);
        self
    }

    pub fn effort(&mut self, effort: u8) -> &mut Self {
        self.effort = Some(effort);
        self
    }

    pub fn threads(&mut self, threads: usize) -> &mut Self {
        self.threads = Some(threads);
        self
    }

    pub fn lossless_jpeg(&mut self, enabled: bool) -> &mut Self {
        self.lossless_jpeg = enabled;
        if enabled {
            self.distance = Some(0.0);
        }
        self
    }

    pub fn allow_jpeg_reconstruction(&mut self, allow: bool) -> &mut Self {
        self.allow_jpeg_recon = Some(allow);
        self
    }

    pub fn cicp<S: AsRef<str>>(&mut self, cicp: S) -> &mut Self {
        self.cicp = Some(cicp.as_ref().to_string());
        self
    }

    pub fn icc_profile<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.icc_profile = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn apple_compat(&mut self, enabled: bool) -> &mut Self {
        self.apple_compat = enabled;
        self
    }

    pub fn use_stdin(&mut self, enabled: bool) -> &mut Self {
        self.use_stdin = enabled;
        self
    }

    pub fn intensity_target(&mut self, target: f32) -> &mut Self {
        self.intensity_target = Some(target);
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_CJXL);

        if let Some(d) = self.distance {
            cmd.arg(constants::JXL_ARG_DISTANCE).arg(format!("{d:.2}"));
        }

        if let Some(e) = self.effort {
            cmd.arg(constants::JXL_ARG_EFFORT).arg(e.to_string());
        }

        if let Some(j) = self.threads {
            cmd.arg(constants::JXL_ARG_THREADS).arg(j.to_string());
        }

        if self.lossless_jpeg {
            cmd.arg(constants::JXL_ARG_LOSSLESS_JPEG);
        }

        if let Some(allow) = self.allow_jpeg_recon {
            cmd.arg(format!("{}={}", constants::JXL_ARG_ALLOW_JPEG_RECON, if allow { "1" } else { "0" }));
        }

        if let Some(cicp) = &self.cicp {
            cmd.arg(format!("{}={}", constants::JXL_ARG_CICP, cicp));
        }

        if let Some(icc) = &self.icc_profile {
            cmd.arg("-x").arg(format!("icc_pathname={}", icc.display()));
        }

        if self.apple_compat {
            cmd.arg(constants::JXL_ARG_COMPRESS_BOXES);
        }

        if let Some(it) = self.intensity_target {
            cmd.arg("--intensity_target").arg(format!("{it:.2}"));
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        cmd.arg("--");

        if self.use_stdin {
            cmd.arg("-");
            cmd.stdin(Stdio::piped());
        } else if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg(crate::safe_path_arg(output).as_ref());
        } else {
            // For piping to stdout, but cjxl usually expects a filename or '-'
            cmd.arg("-");
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(constants::TOOL_CJXL).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }
}

/// Builder for constructing `djxl` commands.
#[derive(Debug, Default)]
pub struct DjxlBuilder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    extra_args: Vec<String>,
}

impl DjxlBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
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

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_DJXL);

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg(crate::safe_path_arg(output).as_ref());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(constants::TOOL_DJXL).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }
}

//! Type-safe builders for JPEG XL (cjxl, djxl) tools.

use crate::builder_base::ToolBuilder;
use crate::constants;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Builder for constructing `cjxl` commands.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct CjxlBuilder {
    base: crate::builder_base::BaseBuilder,
    distance: Option<f32>,
    effort: Option<u8>,
    threads: Option<usize>,
    lossless_jpeg: bool,
    allow_jpeg_recon: Option<bool>,
    allow_expert_options: bool,
    cicp: Option<String>,
    icc_profile: Option<PathBuf>,
    apple_compat: bool,
    use_stdin: bool,
    intensity_target: Option<f32>,
}

impl CjxlBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub const fn distance(&mut self, distance: f32) -> &mut Self {
        self.distance = Some(distance);
        self
    }

    pub const fn effort(&mut self, effort: u8) -> &mut Self {
        self.effort = Some(effort);
        self
    }

    pub const fn threads(&mut self, threads: usize) -> &mut Self {
        self.threads = Some(threads);
        self
    }

    pub const fn lossless_jpeg(&mut self, enabled: bool) -> &mut Self {
        self.lossless_jpeg = enabled;
        if enabled {
            self.distance = Some(0.0);
        }
        self
    }

    pub const fn allow_jpeg_reconstruction(&mut self, allow: bool) -> &mut Self {
        self.allow_jpeg_recon = Some(allow);
        self
    }

    pub const fn allow_expert_options(&mut self, allow: bool) -> &mut Self {
        self.allow_expert_options = allow;
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

    pub const fn apple_compat(&mut self, enabled: bool) -> &mut Self {
        self.apple_compat = enabled;
        self
    }

    pub const fn use_stdin(&mut self, enabled: bool) -> &mut Self {
        self.use_stdin = enabled;
        self
    }

    pub const fn intensity_target(&mut self, target: f32) -> &mut Self {
        self.intensity_target = Some(target);
        self
    }
}

crate::impl_base_builder_accessors_full!(CjxlBuilder);

impl ToolBuilder for CjxlBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_CJXL
    }

    /// Construct the `std::process::Command`.
    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if self.use_stdin {
            cmd.arg("-");
        } else if let Some(input) = self.base.inputs.first() {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.base.output {
            cmd.arg(crate::safe_path_arg(output).as_ref());
        } else {
            cmd.arg("-");
        }

        if let Some(d) = self.distance
            && !self.lossless_jpeg
        {
            cmd.arg(constants::JXL_ARG_DISTANCE).arg(format!("{d}"));
        }

        if let Some(e) = self.effort {
            let lossless_encode_e11 =
                self.lossless_jpeg && e == constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT;
            let expert_e11 =
                self.allow_expert_options && e == constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT;
            debug_assert!(
                constants::is_supported_jxl_effort_with_expert(e, self.allow_expert_options)
                    || lossless_encode_e11,
                "unsupported cjxl effort {e}; runtime policy permits e7/e8/e11 by default, e11 \
                 for JPEG lossless encode, and e11 for explicit expert options; e9 is disabled"
            );
            if lossless_encode_e11 || expert_e11 {
                cmd.arg(constants::JXL_ARG_ALLOW_EXPERT_OPTIONS);
            }
            cmd.arg(constants::JXL_ARG_EFFORT).arg(e.to_string());
        }

        if let Some(j) = self.threads {
            cmd.arg(constants::JXL_ARG_THREADS).arg(j.to_string());
        }

        if self.lossless_jpeg {
            cmd.arg(constants::JXL_ARG_LOSSLESS_JPEG);
        }

        if let Some(allow) = self.allow_jpeg_recon {
            cmd.arg(format!(
                "{}={}",
                constants::JXL_ARG_ALLOW_JPEG_RECON,
                if allow { "1" } else { "0" }
            ));
        }

        if let Some(cicp) = &self.cicp {
            cmd.arg("-x")
                .arg(format!("{}={}", constants::JXL_ARG_COLOR_SPACE, cicp));
        }

        if let Some(icc) = &self.icc_profile {
            cmd.arg("-x").arg(format!(
                "{}={}",
                constants::JXL_ARG_ICC_PATHNAME,
                icc.display()
            ));
        }

        if self.apple_compat {
            cmd.arg(constants::JXL_ARG_COMPRESS_BOXES);
        }

        if let Some(it) = self.intensity_target {
            cmd.arg("--intensity_target").arg(format!("{it:.2}"));
        }

        self.base.apply_to_command(&mut cmd);

        if self.use_stdin {
            cmd.stdin(Stdio::piped());
        }

        cmd
    }
}

/// Builder for constructing `djxl` commands.
#[derive(Debug, Default)]
pub struct DjxlBuilder {
    base: crate::builder_base::BaseBuilder,
}

impl DjxlBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }
}

crate::impl_base_builder_accessors_full!(DjxlBuilder);

impl ToolBuilder for DjxlBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_DJXL
    }

    /// Construct the `std::process::Command`.
    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        self.base.apply_first_input(&mut cmd, None);
        self.base.apply_output(&mut cmd, None);
        self.base.apply_to_command(&mut cmd);

        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cjxl_builder() {
        let mut builder = CjxlBuilder::new();
        builder
            .input(Path::new("in.png"))
            .output(Path::new("out.jxl"))
            .distance(1.5)
            .effort(7)
            .threads(4)
            .lossless_jpeg(true)
            .allow_jpeg_reconstruction(false)
            .cicp("1")
            .apple_compat(true)
            .intensity_target(250.0);

        let cmd = builder.build();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| {
                a.to_str().unwrap_or_else(|| {
                    unreachable!(
                        "CRITICAL: Command argument contains invalid UTF-8 in test_cjxl_builder"
                    )
                })
            })
            .collect();

        // lossless_jpeg overrides distance, so -d is omitted
        assert!(!args.contains(&"-d"));

        assert!(args.contains(&"-e"));
        assert!(args.contains(&"7"));

        assert!(args.contains(&"--num_threads"));
        assert!(args.contains(&"4"));

        assert!(args.contains(&"--lossless_jpeg=1"));
        assert!(args.contains(&"--allow_jpeg_reconstruction=0"));

        assert!(args.contains(&"-x"));
        assert!(args.contains(&"color_space=1"));

        assert!(args.contains(&"--compress_boxes=0"));
        assert!(args.contains(&"--intensity_target"));
        assert!(args.contains(&"250.00"));
    }

    #[test]
    fn cjxl_builder_allows_e11_for_lossless_jpeg_encode_without_expert_options() {
        let mut builder = CjxlBuilder::new();
        builder
            .input(Path::new("in.jpg"))
            .output(Path::new("out.jxl"))
            .lossless_jpeg(true)
            .effort(constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT);

        let cmd = builder.build();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"-e".to_string()));
        assert!(args.contains(&"11".to_string()));
        assert!(args.contains(&"--allow_expert_options".to_string()));
        assert!(args.contains(&"--lossless_jpeg=1".to_string()));
    }

    #[test]
    fn cjxl_builder_accepts_e11_as_ultimate_effort() {
        // e11 is now JXL_ULTIMATE_EFFORT, supported by default
        let mut builder = CjxlBuilder::new();
        builder
            .input(Path::new("in.png"))
            .output(Path::new("out.jxl"))
            .effort(constants::JXL_ULTIMATE_EFFORT);

        let _cmd = builder.build(); // Should not panic
    }

    #[test]
    fn cjxl_builder_emits_waiver_for_explicit_expert_e11() {
        let mut builder = CjxlBuilder::new();
        builder
            .input(Path::new("in.png"))
            .output(Path::new("out.jxl"))
            .allow_expert_options(true)
            .effort(constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT);

        let cmd = builder.build();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"-e".to_string()));
        assert!(args.contains(&"11".to_string()));
        assert!(args.contains(&"--allow_expert_options".to_string()));
    }

    /// CONTRACT: `--compress_boxes=0` only when `apple_compat` is enabled
    /// (Brotli-safe JXL).
    #[test]
    fn contract_cjxl_compress_boxes_gated_by_apple_compat() {
        let mut off = CjxlBuilder::new();
        off.input(Path::new("in.png"))
            .output(Path::new("out.jxl"))
            .apple_compat(false);
        let args_off: Vec<_> = off
            .build()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            !args_off
                .iter()
                .any(|a| a == constants::JXL_ARG_COMPRESS_BOXES),
            "CONTRACT: compress_boxes must be absent without apple_compat"
        );

        let mut on = CjxlBuilder::new();
        on.input(Path::new("in.png"))
            .output(Path::new("out.jxl"))
            .apple_compat(true);
        let args_on: Vec<_> = on
            .build()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args_on
                .iter()
                .any(|a| a == constants::JXL_ARG_COMPRESS_BOXES),
            "CONTRACT: compress_boxes must be present with apple_compat"
        );
    }

    #[test]
    fn test_cjxl_builder_stdin() {
        let mut builder = CjxlBuilder::new();
        builder.use_stdin(true).output(Path::new("out.jxl"));

        let cmd = builder.build();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| {
                a.to_str().unwrap_or_else(|| {
                    unreachable!(
                        "CRITICAL: Command argument contains invalid UTF-8 in \
                         test_cjxl_builder_stdin"
                    )
                })
            })
            .collect();

        assert_eq!(args[0], "-");
        assert_eq!(args[1], "out.jxl");
    }

    #[test]
    fn test_djxl_builder() {
        let mut builder = DjxlBuilder::new();
        builder
            .input(Path::new("in.jxl"))
            .output(Path::new("out.png"));

        let cmd = builder.build();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| {
                a.to_str().unwrap_or_else(|| {
                    unreachable!(
                        "CRITICAL: Command argument contains invalid UTF-8 in test_djxl_builder"
                    )
                })
            })
            .collect();

        assert_eq!(args[0], "in.jxl");
        assert_eq!(args[1], "out.png");
    }
}

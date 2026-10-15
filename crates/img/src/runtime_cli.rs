#![expect(
    clippy::redundant_pub_crate,
    reason = "CLI types must stay internal; public visibility conflicts with unreachable_pub"
)]

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use foundation::infra::runtime_config::{
    self, FallbackPolicy, LoadedConfig, ToolPolicy, photos_args::PhotosArgs,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Args, Debug, Default)]
pub(super) struct RuntimeArgs {
    /// Overlay an explicit JSON configuration after user and project preferences.
    #[arg(long, global = true, conflicts_with = "no_config")]
    config: Option<PathBuf>,
    /// Ignore configuration files; explicit flags and legacy environment still apply.
    #[arg(long, global = true)]
    no_config: bool,
    /// Permit optional image-quality database lookup (requires quality heuristic).
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    allow_database: Option<bool>,
    /// Enable optional image-quality inference. Use =false to override a saved preference.
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    quality_heuristic: Option<bool>,
    /// Standard IMG only: enable the configured pure-media growth budget; verification is unchanged.
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    allow_size_tolerance: Option<bool>,
    /// Standard IMG only: require no pure-media growth, overriding the saved setting.
    #[arg(long, global = true)]
    no_allow_size_tolerance: bool,
    /// Standard IMG only: maximum pure-media growth in bytes when tolerance is enabled.
    #[arg(long, global = true)]
    size_tolerance_bytes: Option<u64>,
    /// Fixed AVIF quality for Fast IMG AVIF; progressive AVIF requires Q10 or higher.
    #[arg(long, global = true, value_parser = clap::value_parser!(u8).range(10..=100))]
    avif_quality: Option<u8>,
    /// Final AVIF encoder speed for Fast IMG AVIF, in the backend's 0..10 range.
    #[arg(long, global = true, value_parser = clap::value_parser!(u8).range(0..=10))]
    avif_speed: Option<u8>,
    /// JPEG retries and intermediate recovery; integrity verification always remains required.
    #[arg(long, global = true, value_enum)]
    fallback_policy: Option<FallbackPolicy>,
    /// Whether alternate encoding/recovery tools may be attempted.
    #[arg(long, global = true, value_enum)]
    tool_policy: Option<ToolPolicy>,
    /// Effort for reversible JPEG-to-JXL encoding; independent of direct pixel JXL effort.
    #[arg(long, global = true, value_parser = clap::value_parser!(u8).range(1..=11))]
    jpeg_effort: Option<u8>,
    /// Compare verified JPEG-reconstruction candidates at different efforts.
    #[arg(long, global = true, value_enum)]
    jpeg_effort_search: Option<runtime_config::EffortSearch>,
    /// Override distance for media where the private policy permits lossy encoding.
    #[arg(long, global = true, value_parser = runtime_config::parse_jxl_distance)]
    jxl_distance: Option<f32>,
    /// Fixed effort for direct pixel JXL encoding, or the highest candidate when searching.
    #[arg(long, global = true, value_parser = runtime_config::parse_direct_jxl_effort)]
    jxl_effort: Option<u8>,
    /// Compare verified direct encodes at fixed quality before distance exploration.
    #[arg(long, global = true, value_enum)]
    jxl_effort_search: Option<runtime_config::EffortSearch>,
    /// Quality-parameter search, independent of source inference and effort search.
    #[arg(long, global = true, value_enum)]
    quality_search: Option<runtime_config::QualitySearch>,
    #[command(flatten)]
    photos: PhotosArgs,
    #[command(flatten)]
    performance: runtime_config::PerformanceArgs,
    #[command(flatten)]
    execution: runtime_config::ExecutionArgs,
    #[command(flatten)]
    cache: runtime_config::cache_args::CacheArgs,
    #[command(flatten)]
    tools: runtime_config::ToolsArgs,
}

#[derive(Subcommand)]
pub(super) enum ConfigCommand {
    /// Show resolved preferences and the source of every value without processing media.
    Show {
        /// Accepted for clarity; show always prints the effective configuration.
        #[arg(long)]
        effective: bool,
    },
    /// Validate the effective configuration, without processing or accessing Photos.
    Validate,
    /// List configuration paths in precedence order without loading them.
    Path,
    /// Create a default configuration at an explicit path; never overwrite an existing file.
    Init { path: PathBuf },
}

impl RuntimeArgs {
    pub(crate) fn validate_fast_strategy(&self, strategy: &str) -> Result<()> {
        anyhow::ensure!(
            self.jxl_distance.is_none(),
            "Fast IMG does not use JXL pixel distance; JPEG reconstruction is a separate primitive"
        );
        anyhow::ensure!(
            self.jxl_effort.is_none() && self.jxl_effort_search.is_none(),
            "Fast IMG does not directly encode JXL pixels; use JPEG effort for JXL strategy"
        );
        anyhow::ensure!(
            strategy != "avif" || (self.jpeg_effort.is_none() && self.jpeg_effort_search.is_none()),
            "JPEG effort settings do not apply to Fast IMG AVIF"
        );
        anyhow::ensure!(
            strategy != "jxl" || self.quality_search.is_none(),
            "Fast IMG JXL reconstructs exact JPEG bytes and has no quality search"
        );
        anyhow::ensure!(
            strategy != "jxl" || (self.avif_quality.is_none() && self.avif_speed.is_none()),
            "AVIF quality and speed settings apply only to Fast IMG AVIF"
        );
        Ok(())
    }

    pub(crate) fn validate_avif_settings_scope(&self, fast_avif_or_config: bool) -> Result<()> {
        anyhow::ensure!(
            fast_avif_or_config || (self.avif_quality.is_none() && self.avif_speed.is_none()),
            "AVIF quality and speed flags require Fast IMG AVIF or img config"
        );
        Ok(())
    }

    pub(crate) fn inspect(&self, command: &ConfigCommand) -> Result<()> {
        match command {
            ConfigCommand::Show { .. } => println!("{}", self.resolve(false)?.to_json(true)?),
            ConfigCommand::Validate => {
                let loaded = self.resolve(false)?;
                println!(
                    "{}",
                    serde_json::json!({"valid": true, "sources": loaded.sources})
                );
            }
            ConfigCommand::Path => println!(
                "{}",
                serde_json::json!({
                    "user": runtime_config::user_config_path(),
                    "project": runtime_config::project_config_path()?,
                    "explicit": self.config,
                    "files_enabled": !self.no_config,
                    "precedence": ["default", "environment", "user", "project", "explicit", "CLI"]
                })
            ),
            ConfigCommand::Init { path } => {
                use std::io::Write;
                let mut content =
                    serde_json::to_vec_pretty(&runtime_config::RuntimeConfig::default())?;
                content.push(b'\n');
                let target = std::path::absolute(path)
                    .with_context(|| format!("resolve configuration path {}", path.display()))?;
                let parent = target.parent().with_context(|| {
                    format!("configuration path has no parent: {}", target.display())
                })?;
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                file.write_all(&content)?;
                file.as_file().sync_all()?;
                file.persist_noclobber(&target).with_context(|| {
                    format!(
                        "create configuration {} (will not overwrite)",
                        path.display()
                    )
                })?;
                println!("{}", serde_json::json!({"created": path}));
            }
        }
        Ok(())
    }
}

fn apply<T: Copy>(
    value: Option<T>,
    target: &mut T,
    sources: &mut BTreeMap<String, String>,
    source_chain: &mut BTreeMap<String, Vec<String>>,
    key: &str,
) {
    if let Some(value) = value {
        *target = value;
        runtime_config::record_source(sources, source_chain, key, "CLI");
    }
}

impl RuntimeArgs {
    pub(crate) fn validate_size_tolerance_scope(&self, standard_or_config: bool) -> Result<()> {
        anyhow::ensure!(
            standard_or_config
                || (self.allow_size_tolerance.is_none()
                    && !self.no_allow_size_tolerance
                    && self.size_tolerance_bytes.is_none()),
            "size tolerance flags require img run or config; Fast IMG does not use this policy"
        );
        Ok(())
    }

    pub(crate) fn resolve(&self, legacy_expert: bool) -> Result<LoadedConfig> {
        let mut loaded = runtime_config::load(self.config.as_deref(), self.no_config)?;
        let config = &mut loaded.config;
        let sources = &mut loaded.sources;
        let source_chain = &mut loaded.source_chain;
        if legacy_expert
            && sources
                .get("img.fallback_policy")
                .is_some_and(|source| source == "default")
        {
            config.img.fallback_policy = FallbackPolicy::Repair;
            runtime_config::record_source(
                sources,
                source_chain,
                "img.fallback_policy",
                "CLI --allow_expert_options",
            );
        }
        apply(
            self.allow_database,
            &mut config.img.allow_database,
            sources,
            source_chain,
            "img.allow_database",
        );
        apply(
            self.quality_heuristic,
            &mut config.img.quality_heuristic,
            sources,
            source_chain,
            "img.quality_heuristic",
        );
        apply(
            if self.no_allow_size_tolerance {
                Some(false)
            } else {
                self.allow_size_tolerance
            },
            &mut config.img.allow_size_tolerance,
            sources,
            source_chain,
            "img.allow_size_tolerance",
        );
        apply(
            self.size_tolerance_bytes,
            &mut config.img.size_tolerance_bytes,
            sources,
            source_chain,
            "img.size_tolerance_bytes",
        );
        apply(
            self.avif_quality,
            &mut config.img.avif_quality,
            sources,
            source_chain,
            "img.avif_quality",
        );
        apply(
            self.avif_speed,
            &mut config.img.avif_speed,
            sources,
            source_chain,
            "img.avif_speed",
        );
        apply(
            self.fallback_policy,
            &mut config.img.fallback_policy,
            sources,
            source_chain,
            "img.fallback_policy",
        );
        apply(
            self.tool_policy,
            &mut config.tools.policy,
            sources,
            source_chain,
            "tools.policy",
        );
        apply(
            self.jpeg_effort,
            &mut config.img.jpeg_effort,
            sources,
            source_chain,
            "img.jpeg_effort",
        );
        apply(
            self.jpeg_effort_search,
            &mut config.img.jpeg_effort_search,
            sources,
            source_chain,
            "img.jpeg_effort_search",
        );
        apply(
            self.jxl_distance.map(Some),
            &mut config.img.jxl_distance,
            sources,
            source_chain,
            "img.jxl_distance",
        );
        apply(
            self.jxl_effort.map(Some),
            &mut config.img.jxl_effort,
            sources,
            source_chain,
            "img.jxl_effort",
        );
        apply(
            self.jxl_effort_search,
            &mut config.img.jxl_effort_search,
            sources,
            source_chain,
            "img.jxl_effort_search",
        );
        apply(
            self.quality_search,
            &mut config.img.quality_search,
            sources,
            source_chain,
            "img.quality_search",
        );
        self.photos.apply_to(&mut loaded);
        self.performance.apply_to(&mut loaded);
        self.execution.apply_to(&mut loaded);
        self.cache.apply_to(&mut loaded);
        self.tools.apply_to(&mut loaded)?;
        loaded.config.validate()?;
        Ok(loaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn default_run_preserves_the_private_mode_policy() -> Result<()> {
        let cli = crate::Cli::try_parse_from(["img", "run", "/unused", "--no-config"])?;
        let crate::Commands::Run {
            ultimate,
            archive,
            explore,
            match_quality,
            ..
        } = cli.command
        else {
            unreachable!();
        };
        assert!(!ultimate && !archive && explore && match_quality);
        let loaded = cli.policy.resolve(false)?;
        assert_eq!(loaded.config.img.jxl_distance, None);
        assert_eq!(loaded.config.img.jxl_effort, None);
        assert_eq!(loaded.config.img.avif_quality, 100);
        assert_eq!(loaded.config.img.avif_speed, 0);
        assert_eq!(
            loaded.config.img.jxl_effort_search,
            runtime_config::EffortSearch::Fixed
        );
        assert_eq!(
            loaded.config.img.jpeg_effort_search,
            runtime_config::EffortSearch::Fixed
        );
        assert_eq!(
            loaded.config.img.quality_search,
            runtime_config::QualitySearch::ModeDefault
        );
        Ok(())
    }

    #[test]
    fn fast_img_avif_values_layer_independently_of_quality_search() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config = temp.path().join("image.json");
        std::fs::write(
            &config,
            br#"{"config_version":1,"img":{"avif_quality":85,"avif_speed":4,"quality_search":"adaptive"}}"#,
        )?;
        let path = config.to_str().context("test configuration path")?;
        let cli = crate::Cli::try_parse_from([
            "img",
            "fast-img",
            "/unused",
            "--strategy",
            "avif",
            "--config",
            path,
            "--avif-quality",
            "10",
            "--avif-speed",
            "10",
        ])?;
        let loaded = cli.policy.resolve(false)?;
        assert_eq!(loaded.config.img.avif_quality, 10);
        assert_eq!(loaded.config.img.avif_speed, 10);
        assert_eq!(
            loaded.config.img.quality_search,
            runtime_config::QualitySearch::Adaptive
        );
        assert_eq!(loaded.sources["img.avif_quality"], "CLI");
        assert_eq!(loaded.sources["img.avif_speed"], "CLI");
        assert_eq!(
            loaded.source_chain["img.avif_quality"],
            ["default", path, "CLI"]
        );
        assert_eq!(loaded.sources["img.quality_search"], path);
        assert!(cli.policy.validate_fast_strategy("avif").is_ok());
        assert!(cli.policy.validate_avif_settings_scope(true).is_ok());
        Ok(())
    }

    #[test]
    fn fast_avif_flags_are_scoped_before_processing() -> Result<()> {
        for flag in ["--avif-quality", "--avif-speed"] {
            let run = crate::Cli::try_parse_from(["img", "run", "/unused", flag, "10"])?;
            assert!(run.policy.validate_avif_settings_scope(false).is_err());

            let fast_jxl = crate::Cli::try_parse_from([
                "img",
                "fast-img",
                "/unused",
                "--strategy",
                "jxl",
                flag,
                "10",
            ])?;
            assert!(fast_jxl.policy.validate_fast_strategy("jxl").is_err());

            let config = crate::Cli::try_parse_from(["img", "config", "show", flag, "10"])?;
            assert!(config.policy.validate_avif_settings_scope(true).is_ok());
        }
        for (flag, invalid) in [
            ("--avif-quality", "0"),
            ("--avif-quality", "9"),
            ("--avif-quality", "101"),
            ("--avif-speed", "11"),
        ] {
            assert!(crate::Cli::try_parse_from(["img", "config", "show", flag, invalid]).is_err());
        }
        Ok(())
    }

    #[test]
    fn fast_search_flags_reject_inapplicable_encoder_scopes() -> Result<()> {
        let fast = crate::Cli::try_parse_from([
            "img",
            "fast-img",
            "/unused",
            "--jxl-effort-search",
            "adaptive",
        ])?;
        assert!(fast.policy.validate_fast_strategy("jxl").is_err());
        let fast = crate::Cli::try_parse_from([
            "img",
            "fast-img",
            "/unused",
            "--quality-search",
            "fixed",
        ])?;
        assert!(fast.policy.validate_fast_strategy("jxl").is_err());
        fast.policy.validate_fast_strategy("avif")?;
        for distance in ["0", "0.4"] {
            let fast = crate::Cli::try_parse_from([
                "img",
                "fast-img",
                "/unused",
                "--jxl-distance",
                distance,
            ])?;
            assert!(fast.policy.validate_fast_strategy("jxl").is_err());
            assert!(fast.policy.validate_fast_strategy("avif").is_err());
        }
        let fast = crate::Cli::try_parse_from([
            "img",
            "fast-img",
            "/unused",
            "--jpeg-effort-search",
            "adaptive",
        ])?;
        fast.policy.validate_fast_strategy("jxl")?;
        assert!(fast.policy.validate_fast_strategy("avif").is_err());
        Ok(())
    }

    #[test]
    fn search_modes_are_independent_typed_overrides() -> Result<()> {
        let loaded = crate::Cli::try_parse_from([
            "img",
            "config",
            "show",
            "--no-config",
            "--jxl-effort-search",
            "adaptive",
            "--jpeg-effort-search",
            "adaptive",
            "--quality-search",
            "adaptive",
        ])?
        .policy
        .resolve(false)?;
        assert_eq!(
            loaded.config.img.jxl_effort_search,
            runtime_config::EffortSearch::Adaptive
        );
        assert_eq!(
            loaded.config.img.quality_search,
            runtime_config::QualitySearch::Adaptive
        );
        assert_eq!(loaded.config.img.jpeg_effort, 11);
        assert_eq!(
            loaded.config.img.jpeg_effort_search,
            runtime_config::EffortSearch::Adaptive
        );
        for key in [
            "img.jxl_effort_search",
            "img.jpeg_effort_search",
            "img.quality_search",
        ] {
            assert_eq!(loaded.sources[key], "CLI");
        }
        assert!(
            crate::Cli::try_parse_from(["img", "config", "show", "--quality-search", "automatic"])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn direct_jxl_effort_flags_preserve_saved_values_and_reconstruction_policy() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config = temp.path().join("direct.json");
        std::fs::write(
            &config,
            br#"{"config_version":1,"img":{"jxl_effort":8,"jpeg_effort":9,"jxl_distance":0.4,"fallback_policy":"strict"}}"#,
        )?;
        let path = config.to_str().context("test configuration path")?;
        for (command, tail) in [
            ("config", "show"),
            ("run", "/unused"),
            ("fast-img", "/unused"),
        ] {
            for (flags, expected, source) in [
                (Vec::new(), 8, path),
                (vec!["--jxl-effort", "7"], 7, "CLI"),
                (vec!["--jxl-effort", "10"], 10, "CLI"),
                (vec!["--jxl-effort", "1"], 1, "CLI"),
                (vec!["--jxl-effort", "11"], 11, "CLI"),
            ] {
                let argv = [vec!["img", command, tail, "--config", path], flags].concat();
                let loaded = crate::Cli::try_parse_from(argv)?.policy.resolve(false)?;
                assert_eq!(loaded.config.img.jxl_effort, Some(expected));
                assert_eq!(loaded.config.img.jpeg_effort, 9);
                assert_eq!(loaded.config.img.jxl_distance, Some(0.4));
                assert_eq!(loaded.sources["img.jxl_effort"], source);
                assert_eq!(
                    loaded.source_chain["img.jxl_effort"]
                        .last()
                        .map(String::as_str),
                    Some(source)
                );
            }
        }
        for effort in ["0", "12", "255", "256", "null"] {
            assert!(
                crate::Cli::try_parse_from(["img", "config", "show", "--jxl-effort", effort])
                    .is_err()
            );
        }
        let loaded = crate::Cli::try_parse_from(["img", "config", "show", "--no-config"])?
            .policy
            .resolve(false)?;
        assert_eq!(loaded.config.img.jxl_effort, None);
        assert_eq!(loaded.config.img.jxl_distance, None);
        let loaded = crate::Cli::try_parse_from(["img", "config", "show", "--config", path])?
            .policy
            .resolve(true)?;
        assert_eq!(loaded.config.img.fallback_policy, FallbackPolicy::Strict);
        assert_eq!(loaded.sources["img.fallback_policy"], path);
        assert_eq!(loaded.config.img.jxl_distance, Some(0.4));
        assert_eq!(loaded.config.img.jxl_effort, Some(8));
        assert_eq!(loaded.sources["img.jxl_distance"], path);
        assert_eq!(loaded.sources["img.jxl_effort"], path);
        for distance in ["-0.1", "25.1", "NaN", "inf"] {
            assert!(
                crate::Cli::try_parse_from(["img", "config", "show", "--jxl-distance", distance,])
                    .is_err()
            );
        }
        for distance in ["0", "25"] {
            let cli = crate::Cli::try_parse_from([
                "img",
                "config",
                "show",
                "--config",
                path,
                "--jxl-distance",
                distance,
            ])?;
            let loaded = cli.policy.resolve(false)?;
            assert_eq!(
                loaded.config.img.jxl_distance,
                Some(distance.parse::<f32>()?)
            );
            assert_eq!(loaded.sources["img.jxl_distance"], "CLI");
            assert_eq!(
                loaded.source_chain["img.jxl_distance"]
                    .last()
                    .map(String::as_str),
                Some("CLI")
            );
            assert_eq!(loaded.config.img.jxl_effort, Some(8));
        }
        Ok(())
    }

    #[test]
    fn product_flags_do_not_rewrite_typed_parameter_overrides() -> Result<()> {
        for quality in ["mode-default", "fixed", "adaptive"] {
            for product_flag in ["--ultimate", "--archive", "--match-quality", "--explore"] {
                let cli = crate::Cli::try_parse_from([
                    "img",
                    "run",
                    "/unused",
                    "--no-config",
                    product_flag,
                    "--jxl-effort",
                    "7",
                    "--jxl-distance",
                    "0.4",
                    "--quality-search",
                    quality,
                ])?;
                let loaded = cli.policy.resolve(false)?;
                assert_eq!(loaded.config.img.jxl_effort, Some(7));
                assert_eq!(loaded.config.img.jxl_distance, Some(0.4));
                assert_eq!(loaded.sources["img.jxl_effort"], "CLI");
                assert_eq!(loaded.sources["img.jxl_distance"], "CLI");
                assert_eq!(loaded.sources["img.quality_search"], "CLI");
                assert_eq!(
                    loaded.source_chain["img.jxl_effort"]
                        .last()
                        .map(String::as_str),
                    Some("CLI")
                );
                assert_eq!(
                    serde_json::to_value(loaded.config.img.quality_search)?,
                    quality
                );
            }
        }
        Ok(())
    }

    #[test]
    fn shared_tool_paths_reach_image_config_run_and_fast_image_without_execution() -> Result<()> {
        let path = "/synthetic/\u{5de5}\u{5177} nightly=1/cjxl";
        for (command, tail) in [
            ("config", "show"),
            ("run", "/unused"),
            ("fast-img", "/unused"),
        ] {
            let value = format!("cjxl={path}");
            let cli = crate::Cli::try_parse_from([
                "img",
                command,
                tail,
                "--no-config",
                "--tool",
                "djxl=/synthetic/djxl",
                "--tool",
                "cjxl=/synthetic/previous",
                "--tool",
                &value,
            ])?;
            let loaded = cli.policy.resolve(false)?;
            assert_eq!(loaded.config.tools.paths["cjxl"], PathBuf::from(path));
            assert_eq!(
                loaded.config.tools.paths["djxl"],
                PathBuf::from("/synthetic/djxl")
            );
            assert_eq!(loaded.sources["tools.paths.cjxl"], "CLI");
            assert_eq!(
                loaded.source_chain["tools.paths.cjxl"]
                    .last()
                    .map(String::as_str),
                Some("CLI")
            );
        }
        for value in ["CJXL=/synthetic/cjxl", "cjxl=relative", "cjxl="] {
            assert!(
                crate::Cli::try_parse_from(["img", "config", "show", "--tool", value]).is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn execution_flags_reach_image_inspection_and_fast_image() -> Result<()> {
        for (command, tail) in [
            ("config", "show"),
            ("run", "/unused"),
            ("fast-img", "/unused"),
        ] {
            for mode in [
                foundation::BatchErrorMode::LogAndContinue,
                foundation::BatchErrorMode::FailFast,
            ] {
                let cli = crate::Cli::try_parse_from([
                    "img",
                    command,
                    tail,
                    "--no-config",
                    "--error-mode",
                    mode.as_str(),
                ])?;
                let loaded = cli.policy.resolve(false)?;
                assert_eq!(loaded.config.execution.error_mode, mode);
                assert_eq!(loaded.sources["execution.error_mode"], "CLI");
                assert_eq!(
                    loaded.source_chain["execution.error_mode"]
                        .last()
                        .map(String::as_str),
                    Some("CLI")
                );
            }
        }
        assert!(
            crate::Cli::try_parse_from(["img", "config", "show", "--error-mode", "debug"]).is_err()
        );
        Ok(())
    }

    #[test]
    fn cache_flags_reach_image_and_fast_image_runtime() -> Result<()> {
        for command in ["config", "fast-img"] {
            let tail = if command == "config" {
                "show"
            } else {
                "/unused"
            };
            let cli = crate::Cli::try_parse_from([
                "img",
                command,
                tail,
                "--no-config",
                "--cache-max-bytes",
                "4096",
                "--cache-ttl-seconds",
                "60",
            ])?;
            let loaded = cli.policy.resolve(false)?;
            assert_eq!(loaded.config.cache.path_tree_max_bytes, 4096);
            assert_eq!(loaded.config.cache.path_tree_ttl_seconds, 60);
            assert_eq!(
                loaded.source_chain["cache.path_tree_ttl_seconds"],
                ["default", "CLI"]
            );
        }
        Ok(())
    }

    #[test]
    fn standard_size_tolerance_resolves_files_flags_and_explicit_false() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config = temp.path().join("image.json");
        std::fs::write(
            &config,
            br#"{"config_version":1,"img":{"allow_size_tolerance":true,"size_tolerance_bytes":1234}}"#,
        )?;
        let path = config.to_str().context("test configuration path")?;
        for (tail, expected, source) in [
            (Vec::new(), true, path),
            (vec!["--allow-size-tolerance=false"], false, "CLI"),
            (vec!["--no-allow-size-tolerance"], false, "CLI"),
            (
                vec!["--allow-size-tolerance", "--no-allow-size-tolerance"],
                false,
                "CLI",
            ),
        ] {
            let argv = [vec!["img", "run", "/unused", "--config", path], tail].concat();
            let cli = crate::Cli::try_parse_from(argv)?;
            let loaded = cli.policy.resolve(false)?;
            assert_eq!(loaded.config.img.allow_size_tolerance, expected);
            assert_eq!(loaded.sources["img.allow_size_tolerance"], source);
            assert_eq!(loaded.config.img.size_tolerance_bytes, 1234);
            assert_eq!(loaded.sources["img.size_tolerance_bytes"], path);
        }
        for command in ["run", "config", "fast-img"] {
            let target = if command == "config" {
                "show"
            } else {
                "/unused"
            };
            let cli = crate::Cli::try_parse_from([
                "img",
                command,
                target,
                "--no-config",
                "--allow-size-tolerance",
            ])?;
            assert_eq!(
                cli.policy
                    .validate_size_tolerance_scope(command != "fast-img")
                    .is_ok(),
                command != "fast-img"
            );
        }
        assert!(
            !runtime_config::RuntimeConfig::default()
                .img
                .allow_size_tolerance
        );
        assert!(
            crate::Cli::try_parse_from(["img", "config", "show", "--allow-size-tolerance=invalid"])
                .is_err()
        );
        for value in ["0", "4096", "18446744073709551615"] {
            let cli = crate::Cli::try_parse_from([
                "img",
                "config",
                "show",
                "--config",
                path,
                "--size-tolerance-bytes",
                value,
                "--no-allow-size-tolerance",
            ])?;
            let loaded = cli.policy.resolve(false)?;
            assert_eq!(loaded.config.img.size_tolerance_bytes.to_string(), value);
            assert_eq!(loaded.sources["img.size_tolerance_bytes"], "CLI");
            assert!(!loaded.config.img.allow_size_tolerance);
        }
        let fast = crate::Cli::try_parse_from([
            "img",
            "fast-img",
            "/unused",
            "--size-tolerance-bytes",
            "0",
        ])?;
        assert!(fast.policy.validate_size_tolerance_scope(false).is_err());
        for value in ["-1", "1.5", "18446744073709551616"] {
            assert!(
                crate::Cli::try_parse_from([
                    "img",
                    "config",
                    "show",
                    "--size-tolerance-bytes",
                    value,
                ])
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn runtime_flags_override_legacy_expert_without_global_mutation() -> Result<()> {
        let cli = crate::Cli::try_parse_from([
            "img",
            "fast-img",
            "/unused",
            "--no-config",
            "--allow_expert_options",
            "--fallback-policy",
            "strict",
            "--tool-policy",
            "single",
            "--allow-database=false",
            "--quality-heuristic=false",
            "--jpeg-effort",
            "9",
            "--photos-import-root",
            "Archive",
            "--photos-album-name",
            "Family",
            "--photos-import-batch-size",
            "50",
            "--preserve-folder-structure=false",
            "--photos-verification-batch-size",
            "500",
            "--photos-adaptive-batching",
            "--performance",
            "adaptive",
            "--tool",
            "cjxl=/usr/bin/cjxl",
        ])?;
        let resolved = cli.policy.resolve(true)?;
        assert_eq!(resolved.config.img.fallback_policy, FallbackPolicy::Strict);
        assert_eq!(resolved.config.tools.policy, ToolPolicy::Single);
        assert!(!resolved.config.img.allow_database);
        assert!(!resolved.config.img.quality_heuristic);
        assert_eq!(resolved.config.img.jpeg_effort, 9);
        assert_eq!(
            resolved.config.photos.import_root.as_deref(),
            Some("Archive")
        );
        assert!(!resolved.config.photos.preserve_folder_structure);
        assert_eq!(resolved.config.photos.verification_batch_size, 500);
        assert!(resolved.config.photos.adaptive_batching);
        assert_eq!(resolved.sources["photos.verification_batch_size"], "CLI");
        assert_eq!(resolved.sources["img.fallback_policy"], "CLI");
        assert_eq!(
            resolved.source_chain["img.fallback_policy"],
            ["default", "CLI --allow_expert_options", "CLI"]
        );
        assert_eq!(
            resolved.source_chain["photos.import_batch_size"],
            ["default", "CLI"]
        );
        assert_eq!(
            resolved.source_chain["performance.mode"],
            ["default", "CLI"]
        );
        assert_eq!(resolved.source_chain["tools.paths.cjxl"], ["CLI"]);
        assert!(resolved.sources.iter().all(|(key, source)| {
            resolved
                .source_chain
                .get(key)
                .and_then(|chain| chain.last())
                == Some(source)
        }));
        let shown: serde_json::Value = serde_json::from_str(&resolved.to_json(false)?)?;
        assert_eq!(
            shown["source_chain"]["photos.verification_batch_size"],
            serde_json::json!(["default", "CLI"])
        );
        Ok(())
    }

    #[test]
    fn runtime_flags_reject_invalid_policy_and_effort() {
        for args in [
            vec!["img", "run", "/unused", "--jpeg-effort", "12"],
            vec!["img", "run", "/unused", "--fallback-policy", "anything"],
            vec!["img", "config", "show", "--config", "x", "--no-config"],
        ] {
            assert!(crate::Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn config_init_never_overwrites_and_commands_parse() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("preferences.json");
        let args = RuntimeArgs::default();
        args.inspect(&ConfigCommand::Init { path: path.clone() })?;
        let first = std::fs::read(&path)?;
        assert!(
            args.inspect(&ConfigCommand::Init { path: path.clone() })
                .is_err()
        );
        assert_eq!(std::fs::read(path)?, first);
        assert!(
            args.inspect(&ConfigCommand::Init {
                path: PathBuf::new()
            })
            .is_err()
        );
        assert!(
            args.inspect(&ConfigCommand::Init {
                path: root.path().join("missing").join("preferences.json"),
            })
            .is_err()
        );
        assert!(!root.path().join("missing").exists());
        for command in ["show", "validate", "path"] {
            assert!(crate::Cli::try_parse_from(["img", "config", command, "--no-config"]).is_ok());
        }
        Ok(())
    }

    #[test]
    fn photos_cli_bounds_and_forwarding_share_the_same_policy() -> Result<()> {
        let cli = crate::Cli::try_parse_from([
            "img",
            "config",
            "show",
            "--no-config",
            "--photos-native-batch-size",
            "200",
            "--photos-native-min-batch-size",
            "100",
            "--photos-native-max-batch-size",
            "400",
            "--photos-target-batch-seconds",
            "5",
            "--photos-adaptive-batching=true",
        ])?;
        let resolved = cli.policy.resolve(false)?;
        assert_eq!(resolved.config.photos.native_min_batch_size, 100);
        assert_eq!(resolved.config.photos.native_max_batch_size, 400);
        assert_eq!(resolved.config.photos.target_batch_seconds, 5);
        assert_eq!(resolved.sources["photos.native_min_batch_size"], "CLI");
        let mut forwarded = vec![
            "img".to_owned(),
            "config".into(),
            "show".into(),
            "--no-config".into(),
        ];
        forwarded.extend(cli.policy.photos.cli_arguments());
        assert_eq!(
            serde_json::to_value(
                crate::Cli::try_parse_from(forwarded)?
                    .policy
                    .resolve(false)?
                    .config
            )?,
            serde_json::to_value(resolved.config)?
        );
        let invalid = crate::Cli::try_parse_from([
            "img",
            "config",
            "validate",
            "--no-config",
            "--photos-native-min-batch-size",
            "400",
            "--photos-native-max-batch-size",
            "100",
        ])?;
        assert!(invalid.policy.resolve(false).is_err());
        Ok(())
    }
}

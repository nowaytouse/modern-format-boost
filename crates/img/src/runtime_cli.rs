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
    /// JPEG retries and intermediate recovery; integrity verification always remains required.
    #[arg(long, global = true, value_enum)]
    fallback_policy: Option<FallbackPolicy>,
    /// Whether alternate encoding/recovery tools may be attempted.
    #[arg(long, global = true, value_enum)]
    tool_policy: Option<ToolPolicy>,
    /// Effort for reversible JPEG-to-JXL encoding; direct pixel encoding keeps its mode policy.
    #[arg(long, global = true, value_parser = clap::value_parser!(u8).range(1..=11))]
    jpeg_effort: Option<u8>,
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
        if legacy_expert {
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
        self.photos.apply_to(&mut loaded);
        self.performance.apply_to(&mut loaded);
        self.execution.apply_to(&mut loaded);
        self.cache.apply_to(&mut loaded);
        self.tools.apply_to(&mut loaded)?;
        Ok(loaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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

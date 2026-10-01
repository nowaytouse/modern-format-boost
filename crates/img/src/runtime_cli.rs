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
    /// Override a tool executable, e.g. --tool cjxl=/path/to/cjxl. May be repeated.
    #[arg(long = "tool", global = true, value_parser = parse_tool)]
    tools: Vec<(String, PathBuf)>,
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
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| std::path::Path::new("."));
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                file.write_all(&content)?;
                file.as_file().sync_all()?;
                file.persist_noclobber(path).with_context(|| {
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

fn parse_tool(value: &str) -> Result<(String, PathBuf), String> {
    let (name, path) = value
        .split_once('=')
        .ok_or_else(|| "expected NAME=PATH".to_owned())?;
    if name.is_empty() || path.is_empty() {
        return Err("tool name and executable path must be nonempty".into());
    }
    Ok((name.to_owned(), PathBuf::from(path)))
}

fn apply<T: Copy>(
    value: Option<T>,
    target: &mut T,
    sources: &mut BTreeMap<String, String>,
    key: &str,
) {
    if let Some(value) = value {
        *target = value;
        sources.insert(key.to_owned(), "CLI".into());
    }
}

impl RuntimeArgs {
    pub(crate) fn resolve(&self, legacy_expert: bool) -> Result<LoadedConfig> {
        let mut loaded = runtime_config::load(self.config.as_deref(), self.no_config)?;
        let config = &mut loaded.config;
        let sources = &mut loaded.sources;
        if legacy_expert {
            config.img.fallback_policy = FallbackPolicy::Repair;
            sources.insert(
                "img.fallback_policy".into(),
                "CLI --allow_expert_options".into(),
            );
        }
        apply(
            self.allow_database,
            &mut config.img.allow_database,
            sources,
            "img.allow_database",
        );
        apply(
            self.quality_heuristic,
            &mut config.img.quality_heuristic,
            sources,
            "img.quality_heuristic",
        );
        apply(
            self.fallback_policy,
            &mut config.img.fallback_policy,
            sources,
            "img.fallback_policy",
        );
        apply(
            self.tool_policy,
            &mut config.tools.policy,
            sources,
            "tools.policy",
        );
        apply(
            self.jpeg_effort,
            &mut config.img.jpeg_effort,
            sources,
            "img.jpeg_effort",
        );
        for (name, path) in &self.tools {
            config.tools.paths.insert(name.clone(), path.clone());
            sources.insert(format!("tools.paths.{name}"), "CLI".into());
        }
        self.photos.apply_to(&mut loaded);
        loaded.config.validate()?;
        Ok(loaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
            "--preserve-folder-structure=false",
            "--photos-verification-batch-size",
            "500",
            "--photos-adaptive-batching",
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

#![expect(
    clippy::redundant_pub_crate,
    reason = "CLI types must stay internal; public visibility conflicts with unreachable_pub"
)]

use anyhow::Result;
use clap::{Args, Subcommand};
use foundation::infra::runtime_config::{
    self, FallbackPolicy, LoadedConfig, PhotosBackend, ToolPolicy,
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
    #[arg(long, global = true, value_enum)]
    photos_backend: Option<PhotosBackend>,
    /// Top-level Photos folder name (one component).
    #[arg(long, global = true)]
    photos_import_root: Option<String>,
    /// Base Photos album name (one component).
    #[arg(long, global = true)]
    photos_album_name: Option<String>,
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    preserve_folder_structure: Option<bool>,
    #[arg(long, global = true, value_parser = clap::value_parser!(usize))]
    photos_native_batch_size: Option<usize>,
    #[arg(long, global = true, value_parser = clap::value_parser!(usize))]
    photos_import_batch_size: Option<usize>,
    /// Independent Photos verification window and query cap.
    #[arg(long, global = true, value_parser = clap::value_parser!(usize))]
    photos_verification_batch_size: Option<usize>,
    /// Adapt native transaction sizes after verified batches using latency and memory pressure.
    #[arg(long, global = true, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
    photos_adaptive_batching: Option<bool>,
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
        apply(
            self.photos_backend,
            &mut config.photos.backend,
            sources,
            "photos.backend",
        );
        if let Some(name) = &self.photos_import_root {
            config.photos.import_root = Some(name.clone());
            sources.insert("photos.import_root".into(), "CLI".into());
        }
        if let Some(name) = &self.photos_album_name {
            config.photos.album_name = Some(name.clone());
            sources.insert("photos.album_name".into(), "CLI".into());
        }
        apply(
            self.preserve_folder_structure,
            &mut config.photos.preserve_folder_structure,
            sources,
            "photos.preserve_folder_structure",
        );
        apply(
            self.photos_native_batch_size,
            &mut config.photos.native_batch_size,
            sources,
            "photos.native_batch_size",
        );
        apply(
            self.photos_import_batch_size,
            &mut config.photos.import_batch_size,
            sources,
            "photos.import_batch_size",
        );
        for (name, path) in &self.tools {
            config.tools.paths.insert(name.clone(), path.clone());
            sources.insert(format!("tools.paths.{name}"), "CLI".into());
        }
        apply(
            self.photos_verification_batch_size,
            &mut config.photos.verification_batch_size,
            sources,
            "photos.verification_batch_size",
        );
        apply(
            self.photos_adaptive_batching,
            &mut config.photos.adaptive_batching,
            sources,
            "photos.adaptive_batching",
        );
        config.validate()?;
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
}

use clap::Parser;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct ApiInfo {
    pub direct_links: Vec<String>,
    pub url_template: String,
    pub media_field: String,
}

#[derive(Debug, Clone, Default)]
pub struct SampleSources {
    pub local_dirs: Vec<String>,
    pub remote_apis: Vec<String>,
    pub selection_strategy: String,
    pub file_quality_filter: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Clone)]
pub struct QualityGroup {
    pub sources: SampleSources,
    pub tier_logic: String,
    pub tier_rules: Vec<serde_json::Map<String, Value>>,
}

impl Default for QualityGroup {
    fn default() -> Self {
        Self {
            sources: SampleSources::default(),
            tier_logic: "ANY".to_string(),
            tier_rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RulesConfig {
    pub strict_unknown_rules: bool,
    pub strict_no_silent_fallbacks: bool,
    pub tier_ambiguous_policy: String,
    pub remote_apis: HashMap<String, ApiInfo>,
    pub static_image: HashMap<String, QualityGroup>,
    pub animated_image: HashMap<String, QualityGroup>,
    pub ingest: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Clone)]
pub struct Sample {
    pub path_or_url: String,
    pub base_label: String,
    pub is_remote: bool,
    pub source: SampleSources,
    pub tier_audit: Option<serde_json::Map<String, Value>>,
}

#[derive(clap::Args, Debug, Clone)]
pub struct PlanFlags {
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub no_loop: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct ExecFlags {
    #[arg(long, hide = true)]
    pub execute: bool,
    #[arg(long)]
    pub use_api: bool,
    #[arg(long)]
    pub background: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct BalanceFlags {
    #[arg(long)]
    pub balance: bool,
    #[arg(long)]
    pub no_balance_complexity: bool,
    #[arg(long)]
    pub balance_include_loop_uncertain: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct AssetsFlags {
    #[arg(long)]
    pub fill_runtime_assets: bool,
    #[arg(long)]
    pub no_fill_runtime_assets: bool,
    #[arg(long)]
    pub verify_after: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct DbFlags {
    #[arg(long)]
    pub reset_db: bool,
    #[arg(long)]
    pub repair_schema: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct MiscFlags {
    #[arg(long)]
    pub install_missing_python_deps: bool,
    #[arg(long, short)]
    pub verbose: bool,
    #[arg(long)]
    pub allow_remote: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct MultiLaneFlags {
    #[arg(long)]
    pub four_lane: bool,
    #[arg(long)]
    pub stop: bool,
    #[arg(long)]
    pub rebuild_dylib: bool,
}

#[derive(clap::Args, Debug, Clone)]
pub struct FinalizeFlags {
    #[arg(long)]
    pub finalize_image_quality_model: bool,
    #[arg(long)]
    pub finalize_loop_intent: bool,
    #[arg(long)]
    pub finalize_all: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct Args {
    #[command(flatten)]
    pub plan: PlanFlags,
    #[command(flatten)]
    pub exec: ExecFlags,
    #[command(flatten)]
    pub balance: BalanceFlags,
    #[command(flatten)]
    pub assets: AssetsFlags,
    #[command(flatten)]
    pub db: DbFlags,
    #[command(flatten)]
    pub misc: MiscFlags,
    #[command(flatten)]
    pub multi: MultiLaneFlags,
    #[command(flatten)]
    pub finalize: FinalizeFlags,

    #[arg(long, value_enum, default_value = "all")]
    pub training_mode: TrainingMode,

    #[arg(long)]
    pub label: Option<String>,

    #[arg(long, default_value = "auto")]
    pub loop_intent_label: String,

    #[arg(long, default_value = "0")]
    pub max_high: usize,

    #[arg(long, default_value = "0")]
    pub max_low: usize,

    #[arg(long, default_value = "0")]
    pub max_loop: usize,

    #[arg(long, default_value = "0")]
    pub max_non_loop: usize,

    #[arg(long)]
    pub lane: Vec<String>,

    #[arg(long)]
    pub log_root: Option<String>,

    #[arg(skip)]
    pub fill_runtime_assets_explicit: bool,
}

impl Args {
    /// Python default: fill runtime assets unless `--no-fill-runtime-assets`.
    /// Match py behavior: None → True (default on), --no-fill → False
    #[must_use]
    pub const fn effective_fill_runtime_assets(&self) -> bool {
        // If user explicitly passed --fill-runtime-assets, honor it
        if self.assets.fill_runtime_assets {
            return true;
        }
        !self.assets.no_fill_runtime_assets
    }
}

#[cfg(test)]
mod args_tests {
    use super::Args;
    use clap::Parser;

    #[test]
    fn fill_runtime_defaults_on_without_flags() {
        let args = Args::try_parse_from(["run_training"]).expect("parse");
        assert!(args.effective_fill_runtime_assets()); // Should default to true
    }

    #[test]
    fn fill_runtime_off_with_no_fill_flag() {
        let args =
            Args::try_parse_from(["run_training", "--no-fill-runtime-assets"]).expect("parse");
        assert!(!args.effective_fill_runtime_assets());
    }

    #[test]
    fn fill_runtime_on_with_explicit_flag() {
        let args = Args::try_parse_from(["run_training", "--fill-runtime-assets"]).expect("parse");
        assert!(args.effective_fill_runtime_assets());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum TrainingMode {
    All,
    Static,
    Loop,
}

#[derive(Debug, Clone, Copy)]
pub struct AssetsState {
    pub saw_image_quality: bool,
    pub saw_loop_samples: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct AssetsActions {
    pub install_deps: bool,
    pub verify_after: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct FillAssetsConfig {
    pub state: AssetsState,
    pub actions: AssetsActions,
    pub training_mode: TrainingMode,
}

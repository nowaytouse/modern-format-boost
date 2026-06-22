//! Training database audit and runtime-asset orchestration (Rust port of `training_pipeline.py`).
//!
//! Python script remains the compatibility reference; this module is the primary
//! implementation for Rust callers (`run_training`, `post_training_closure`, etc.).

mod audit;
mod delegate;
pub mod orchestrate;

pub use audit::{
    LOOP_CLUSTERING_SCENARIOS, QUALITY_REGRESSION_SCENARIOS, SCENARIOS, print_full_report,
    print_loop_clustering_report, print_quality_regression_report, repair_multi_scenario_schema,
    verify_embeddings, verify_fabrication_stock, verify_stack_readiness,
};
pub use delegate::{
    project_root, resolve_connstr, run_training_pipeline_subcommand, training_pipeline_command,
};
pub use orchestrate::{
    finalize_image_quality_model, finalize_loop_intent_assets, finalize_runtime_assets,
    refresh_loop_stats, repair_loop_probe_metadata, run_training_batch,
    show_image_quality_model_paths, train_image_quality_model,
};

pub const DEFAULT_CONNSTR: &str = "postgresql://localhost/modern_format_boost";

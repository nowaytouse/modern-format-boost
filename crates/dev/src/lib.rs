//! Dev-only utilities and regression testing backend.
#![allow(clippy::too_many_lines)]

pub mod media;
pub mod run_training;
pub mod training_pipeline;
pub mod infra {
    pub mod background_detach;
    pub mod config_load;
    pub mod corpus_thresholds;
    pub mod drag_drop;
    pub mod elapsed_spinner;
    pub mod fabrication_policy;
    pub mod fastmode_paths;
    pub mod hardening;
    pub mod log_paths;
    pub mod logger;
    pub mod mfb_cargo_env;
    pub mod performance;
    pub mod process_stream;
    pub mod recovery_collection;
    pub mod rich_panel;
    pub mod signal_handlers;
    pub mod system_checks;
    pub mod terminal_input;
    pub mod training_scan;
    pub mod training_session_audit;
    pub mod ui_tokens;
    pub mod watch_mode;
}

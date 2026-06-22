//! `train` — grouped implementation modules (crate root re-exports via `lib.rs`).

pub mod training_progress;

pub mod training_entry_guard;

pub mod training_tier_audit;

pub mod c_api;

/// Probe loop intent for training balance.
/// Mirrors the C-API `mfb_probe_loop_intent` but runs directly in Rust.
pub mod loop_intent_probe {
    use crate::db::database::probe_loop_training_balance;
    use std::path::Path;

    pub use crate::db::database::LoopTrainingBalanceProbe;

    /// Probe loop vs non-loop intent for a given path.
    pub fn probe(path: &Path) -> anyhow::Result<LoopTrainingBalanceProbe> {
        probe_loop_training_balance(path)
    }
}

//! Four-lane training launcher entry point.
//!
//! Compatibility shim aligned with `crates/dev/scripts/start_training_four.py`.
//!
//! Delegates directly to `run_training --four-lane` which is the canonical
//! four-lane launcher. This bin exists so that menu/app/docs invocations
//! targeting `start_training_four` continue to work without duplicating
//! launcher policy.
//!
//! Usage:
//!   cargo run --locked -p dev --bin `start_training_four` -- [args…]
//!
//! All arguments are forwarded verbatim to the `run_training` bin logic.

use anyhow::{Context, Result};
use clap::Parser;
use dev::run_training::four_lane::run_four_lane_launcher;
use dev::run_training::types::Args;

fn apply_training_scan_defaults() {
    // Mirror run_training::apply_training_scan_defaults
    macro_rules! setenv_default {
        ($key:expr, $val:expr) => {
            if std::env::var($key).unwrap_or_default().trim().is_empty() {
                // SAFETY: called before any threads are spawned
                #[allow(unused_unsafe)]
                unsafe {
                    std::env::set_var($key, $val);
                }
            }
        };
    }
    setenv_default!("MFB_TRAINING_INGEST_PROGRESS", "1");
    setenv_default!("MFB_TRAINING_TIER_AUDIT", "1");
    setenv_default!("MFB_TRAINING_TIER_AUDIT_STREAM", "0");
}

fn main() -> Result<()> {
    apply_training_scan_defaults();

    let mut args = Args::parse();

    // Ensure --four-lane is active: this bin is *only* the four-lane launcher.
    if !args.multi.four_lane {
        args.multi.four_lane = true;
    }

    run_four_lane_launcher(&args).context("four-lane launcher failed")
}

//! Legacy three-lane training entry point.
//!
//! Compatibility shim aligned with
//! `crates/dev/scripts/start_training_three.py`.
//!
//! The canonical four-lane launcher is `start_training_four`. This bin exists
//! purely for backward compatibility: existing invocations that reference
//! `start_training_three` continue to work without any policy duplication.
//!
//! Usage:
//!   cargo run --locked -p dev --bin `start_training_three` -- [args…]
//!
//! All arguments are forwarded verbatim to the four-lane launcher.

use anyhow::{Context, Result};
use clap::Parser;
use dev::run_training::four_lane::run_four_lane_launcher;
use dev::run_training::types::Args;

fn apply_training_scan_defaults() {
    macro_rules! setenv_default {
        ($key:expr, $val:expr) => {
            if std::env::var($key).unwrap_or_default().trim().is_empty() {
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

    // Legacy three-lane entry always activates four-lane mode (the three-lane
    // distinction no longer exists — all launches use the four canonical lanes).
    if !args.multi.four_lane {
        args.multi.four_lane = true;
    }

    run_four_lane_launcher(&args).context("start_training_three: four-lane launcher failed")
}

//! Ingest profile merge + SSOT DB cap enforcement.
//!
//! Mirrors `apply_ingest_profile()` and `enforce_training_db_caps()` in
//! `crates/dev/scripts/run_training.py`.

use super::collect::explicit_loop_balance_bucket;
use super::types::{Args, TrainingMode};
use crate::run_training::types::RulesConfig;
use anyhow::{Result, bail};
use serde_json::Value;

pub const STATIC_QUALITY_DB_CAP_PER_CLASS: usize = 4000;
pub const LOOP_INTENT_DB_CAP_PER_CLASS: usize = 2000;

fn cap_name_to_field(name: &str) -> Option<fn(&mut Args) -> &mut usize> {
    match name {
        "max_high" => Some(|a: &mut Args| &mut a.max_high),
        "max_low" => Some(|a: &mut Args| &mut a.max_low),
        "max_loop" => Some(|a: &mut Args| &mut a.max_loop),
        "max_non_loop" => Some(|a: &mut Args| &mut a.max_non_loop),
        _ => None,
    }
}

fn read_cap(profile: &serde_json::Map<String, Value>, name: &str) -> Result<Option<usize>> {
    let raw = profile.get(name);
    if raw.is_none() {
        return Ok(None);
    }
    match raw {
        Some(Value::Number(n)) => {
            let v = n
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("ingest.{name} must be an integer"))?;
            let v_usize = foundation::numeric_cast::u64_to_usize_strict(v, name)
                .ok_or_else(|| anyhow::anyhow!("ingest.{name} overflowed usize"))?;
            Ok(Some(v_usize))
        }
        Some(_) => bail!("ingest.{name} must be an integer"),
        None => Ok(None),
    }
}

/// Apply `training_rules.local.json` → ingest knobs when CLI left defaults.
pub fn apply_ingest_profile(args: &mut Args, profile: Option<&serde_json::Map<String, Value>>) {
    let Some(profile) = profile else {
        return;
    };
    let mut applied = Vec::new();

    if let Some(Value::String(mode)) = profile.get("training_mode") {
        let mode = mode.trim();
        if matches!(mode, "all" | "static" | "loop") {
            args.training_mode = match mode {
                "static" => TrainingMode::Static,
                "loop" => TrainingMode::Loop,
                _ => TrainingMode::All,
            };
            applied.push(format!("training_mode={mode}"));
        }
    }

    if profile.get("balance").and_then(Value::as_bool) == Some(true) && !args.balance.balance {
        args.balance.balance = true;
        applied.push("balance=on".to_string());
    }

    for name in ["max_high", "max_low", "max_loop", "max_non_loop"] {
        let Ok(Some(cap_val)) = read_cap(profile, name) else {
            continue;
        };
        if cap_val > 0
            && let Some(field) = cap_name_to_field(name)
        {
            let current = *field(args);
            if current == 0 {
                *field(args) = cap_val;
                applied.push(format!("{name}={cap_val}"));
            }
        }
    }

    if profile.get("fill_runtime_assets").and_then(Value::as_bool) == Some(true)
        && !args.fill_runtime_assets_explicit
        && args.assets.no_fill_runtime_assets
    {
        args.assets.no_fill_runtime_assets = false;
        applied.push("fill_runtime_assets=on".to_string());
    }

    if profile
        .get("no_balance_complexity")
        .and_then(Value::as_bool)
        == Some(true)
        && !args.balance.no_balance_complexity
    {
        args.balance.no_balance_complexity = true;
        applied.push("no_balance_complexity=on".to_string());
    }

    if !applied.is_empty() {
        eprintln!(
            "  [INGEST] training_rules.local.json ingest: {}",
            applied.join(", ")
        );
    }
}

pub fn enforce_training_db_caps(args: &mut Args) -> Result<()> {
    let mode = match args.training_mode {
        TrainingMode::All => "all",
        TrainingMode::Static => "static",
        TrainingMode::Loop => "loop",
    };
    let label = args.label.as_deref().unwrap_or("").trim().to_lowercase();
    let loop_bucket = explicit_loop_balance_bucket(&args.loop_intent_label);

    let mut targets: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();

    if matches!(mode, "static" | "all") {
        if label.is_empty() || label == "high" {
            targets.insert("max_high", STATIC_QUALITY_DB_CAP_PER_CLASS);
        }
        if label.is_empty() || label == "low" {
            targets.insert("max_low", STATIC_QUALITY_DB_CAP_PER_CLASS);
        }
    }
    if matches!(mode, "loop" | "all") {
        match loop_bucket.as_deref() {
            Some("loop" | "uncertain") => {
                targets.insert("max_loop", LOOP_INTENT_DB_CAP_PER_CLASS);
            }
            Some("non_loop") => {
                targets.insert("max_non_loop", LOOP_INTENT_DB_CAP_PER_CLASS);
            }
            _ => {
                targets.insert("max_loop", LOOP_INTENT_DB_CAP_PER_CLASS);
                targets.insert("max_non_loop", LOOP_INTENT_DB_CAP_PER_CLASS);
            }
        }
    }

    let mut applied = Vec::new();
    for (cap_name, ceiling) in &targets {
        let cur = match *cap_name {
            "max_high" => args.max_high,
            "max_low" => args.max_low,
            "max_loop" => args.max_loop,
            "max_non_loop" => args.max_non_loop,
            _ => continue,
        };
        if cur != *ceiling {
            if cur > *ceiling {
                applied.push(format!("{cap_name}={cur}→{ceiling}"));
                match *cap_name {
                    "max_high" => args.max_high = *ceiling,
                    "max_low" => args.max_low = *ceiling,
                    "max_loop" => args.max_loop = *ceiling,
                    "max_non_loop" => args.max_non_loop = *ceiling,
                    _ => {}
                }
            } else if cur == 0 {
                applied.push(format!("{cap_name}={ceiling}"));
                match *cap_name {
                    "max_high" => args.max_high = *ceiling,
                    "max_low" => args.max_low = *ceiling,
                    "max_loop" => args.max_loop = *ceiling,
                    "max_non_loop" => args.max_non_loop = *ceiling,
                    _ => {}
                }
            }
        }
    }

    for (cap_name, cap_ceil) in [
        ("max_high", STATIC_QUALITY_DB_CAP_PER_CLASS),
        ("max_low", STATIC_QUALITY_DB_CAP_PER_CLASS),
        ("max_loop", LOOP_INTENT_DB_CAP_PER_CLASS),
        ("max_non_loop", LOOP_INTENT_DB_CAP_PER_CLASS),
    ] {
        if targets.contains_key(cap_name) {
            continue;
        }
        let cur = match cap_name {
            "max_high" => args.max_high,
            "max_low" => args.max_low,
            "max_loop" => args.max_loop,
            "max_non_loop" => args.max_non_loop,
            _ => 0,
        };
        if cur > cap_ceil {
            match cap_name {
                "max_high" => args.max_high = cap_ceil,
                "max_low" => args.max_low = cap_ceil,
                "max_loop" => args.max_loop = cap_ceil,
                "max_non_loop" => args.max_non_loop = cap_ceil,
                _ => {}
            }
            applied.push(format!("{cap_name}={cur}→{cap_ceil}"));
        }
    }

    if mode == "static" && label == "high" {
        for cap_name in ["max_low", "max_loop", "max_non_loop"] {
            zero_cap(args, cap_name, &mut applied);
        }
    } else if mode == "static" && label == "low" {
        for cap_name in ["max_high", "max_loop", "max_non_loop"] {
            zero_cap(args, cap_name, &mut applied);
        }
    } else if mode == "loop" && loop_bucket.as_deref() == Some("loop") {
        for cap_name in ["max_high", "max_low", "max_non_loop"] {
            zero_cap(args, cap_name, &mut applied);
        }
    } else if mode == "loop" && loop_bucket.as_deref() == Some("non_loop") {
        for cap_name in ["max_high", "max_low", "max_loop"] {
            zero_cap(args, cap_name, &mut applied);
        }
    } else if mode == "loop" && loop_bucket.as_deref() == Some("uncertain") {
        for cap_name in ["max_high", "max_low", "max_non_loop"] {
            zero_cap(args, cap_name, &mut applied);
        }
    }

    if !applied.is_empty() {
        eprintln!("  [INGEST] training_db_caps_ssot: {}", applied.join(", "));
    }
    Ok(())
}

fn zero_cap(args: &mut Args, cap_name: &str, applied: &mut Vec<String>) {
    let cur = match cap_name {
        "max_high" => args.max_high,
        "max_low" => args.max_low,
        "max_loop" => args.max_loop,
        "max_non_loop" => args.max_non_loop,
        _ => return,
    };
    if cur != 0 {
        match cap_name {
            "max_high" => args.max_high = 0,
            "max_low" => args.max_low = 0,
            "max_loop" => args.max_loop = 0,
            "max_non_loop" => args.max_non_loop = 0,
            _ => {}
        }
        applied.push(format!("{cap_name}=0"));
    }
}

pub fn validate_ingest_rules(rules: &RulesConfig) -> Result<()> {
    if !rules.strict_unknown_rules {
        bail!("rule_engine.strict_unknown_rules must be true (silent unknown rules are forbidden)");
    }
    if rules.strict_no_silent_fallbacks {
        for required in ["high_quality", "low_quality"] {
            if !rules.static_image.contains_key(required) {
                bail!("static_image.{required} is required when strict_no_silent_fallbacks=true");
            }
        }
    }
    Ok(())
}

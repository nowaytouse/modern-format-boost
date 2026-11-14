//! Database Diagnostics CLI
//!
//! Prints inference log summary, feature discriminative power analysis,
//! and blind spot reports to help calibrate the decision tree.
//!
//! Usage: cargo run --bin `db_diagnostics`

use foundation::Rational;
use foundation::database::{
    open_pg_client, query_feature_discriminative_power, query_inference_blind_spots,
    query_inference_log_summary,
};
use foundation::media_conversion_gate;
use foundation::modern_ui::symbols::{self, pick};
use foundation::multi_scenario_db::init_multi_scenario_schema;
use foundation::{log_detail, log_info, log_section};

type Result<T> = core::result::Result<T, anyhow::Error>;

fn main() -> Result<()> {
    foundation::entry_guard::assert_pipeline_tool_entry("db_diagnostics")?;
    foundation::log_summary_header!(
        foundation::infra::static_logs::messages::LABEL_SESSION_SUMMARY
    );
    log_detail!(
        "Starting comprehensive database feedback loop diagnostics and inference calibration..."
    );

    let mut conn = open_pg_client()?;
    init_multi_scenario_schema(&mut conn)?;

    // ── Section 1: Inference Log Summary ──
    print_inference_summary(&mut conn)?;

    // ── Section 2: Feature Discriminative Power (Level 1) ──
    print_discriminative_power(&mut conn)?;

    // ── Section 3: Blind Spot Discovery (Level 3) ──
    print_blind_spots(&mut conn)?;

    log_detail!("");
    log_detail!("═══════════════════════════════════════════════════════════════");
    log_info!(
        foundation::infra::static_logs::messages::LABEL_STRATEGY,
        "{} Tip: Run assessments on more files to accumulate inference logs.",
        pick(symbols::INFO, symbols::plain::INFO),
    );
    log_detail!("   Once enough data is collected, blind spots and weight recommendations");
    log_detail!("   will become increasingly reliable.");

    Ok(())
}

/// Prints a summary of the inference logs.
///
/// # Errors
/// Returns an error if the database query fails.
fn print_inference_summary(conn: &mut postgres::Client) -> Result<()> {
    log_section!(foundation::infra::static_logs::messages::LABEL_INFERENCE_AUDIT);

    let summary = query_inference_log_summary(conn)?;

    if summary.total_records == 0 {
        log_info!(
            foundation::infra::static_logs::messages::LABEL_SESSION_SUMMARY,
            "{} No inference records yet. Pipeline dormant.",
            pick(symbols::WARNING, symbols::plain::WARNING),
        );
        log_detail!(
            "      Run assess_loop_intent_from_meta on some files to start accumulating data."
        );
        log_detail!("");
        return Ok(());
    }

    log_detail!(
        "   Total inferences logged: {} (Decision Engine History)",
        summary.total_records
    );
    let permille = {
        let ratio = Rational::from(summary.layer7_fallback_count)
            / Rational::from(summary.total_records.max(1));
        let res = ratio * Rational::from(10_000_i32);
        res.to_f64()
    };
    log_detail!(
        "   Layer 7 fallbacks:       {} ({:.1}% - structural uncertainty rate)",
        summary.layer7_fallback_count,
        permille / 100.0_f64
    );

    if let Some(avg_tree) = summary.avg_tree_probability {
        log_detail!("   Avg tree probability:    {avg_tree:.3}");
    }
    if let Some(avg_knn) = summary.avg_knn_confidence {
        log_detail!("   Avg KNN confidence:      {avg_knn:.3}");
    }
    if let Some(avg_final) = summary.avg_final_probability {
        log_detail!("   Avg final probability:   {avg_final:.3}");
    }

    log_detail!("");
    log_detail!("   Verdict Distribution Audit:");
    for (verdict, count) in &summary.verdict_counts {
        let ratio = Rational::from(*count) / Rational::from(summary.total_records.max(1));
        let pct = ratio.to_f64() * 100.0_f64;
        if let Some(bar_len) =
            foundation::numeric_cast::f64_to_usize_strict(pct / 5.0, "verdict_ratio")
        {
            let bar = "█".repeat(bar_len);
            log_detail!("     {verdict:<14} {count:>5} ({pct:>5.1}%) {bar}");
        } else {
            log_detail!("     {verdict:<14} {count:>5} ({pct:>5.1}%) [ANOMALY]");
        }
    }

    log_detail!("");
    log_detail!("   Layer Exit Distribution Audit:");
    for (layer, count) in &summary.layer_exit_counts {
        let ratio = Rational::from(*count) / Rational::from(summary.total_records.max(1));
        let pct = ratio.to_f64() * 100.0_f64;
        log_detail!("     {layer:<40} {count:>5} ({pct:>5.1}%)");
    }

    log_detail!("");
    Ok(())
}

/// Prints the discriminative power of each feature.
///
/// # Errors
/// Returns an error if the database query fails.
fn print_discriminative_power(conn: &mut postgres::Client) -> Result<()> {
    log_section!(foundation::infra::static_logs::messages::LABEL_FEATURE_AUDIT);

    let features = query_feature_discriminative_power(conn)?;

    if features.is_empty() {
        log_info!(
            foundation::infra::static_logs::messages::LABEL_SESSION_SUMMARY,
            "{} Not enough labeled samples to compute discriminative power.",
            pick(symbols::WARNING, symbols::plain::WARNING),
        );
        log_detail!("      Need both 'high' (loop) and 'video' (non-loop) labeled samples.");
        log_detail!("");
        return Ok(());
    }

    log_detail!(
        "   {:<28} {:>12} {:>12} {:>12} {:>6}",
        "Discriminative Feature",
        "Mean(Loop)",
        "Mean(Video)",
        "Discrim.",
        "Samples"
    );

    for f in &features {
        let strong_str = match f.mean_loop_strong {
            None => "   N/A".to_string(),
            Some(v) => format!("{v:>12.4}"),
        };
        let weak_str = match f.mean_loop_weak {
            None => "   N/A".to_string(),
            Some(v) => format!("{v:>12.4}"),
        };

        let indicator = if f.discriminative_power.abs() > 0.5_f64 {
            "★"
        } else if f.discriminative_power.abs() > 0.2_f64 {
            "○"
        } else {
            "·"
        };

        log_detail!(
            " {indicator} {:<28} {strong_str} {weak_str} {:>12.4} {:>6}",
            f.feature_name,
            f.discriminative_power,
            f.sample_count
        );
    }

    // Print actionable recommendations
    log_detail!("");
    let strong_features: Vec<_> = features
        .iter()
        .filter(|f| f.discriminative_power.abs() > 0.5_f64)
        .collect();
    let weak_features: Vec<_> = features
        .iter()
        .filter(|f| f.discriminative_power.abs() < 0.1_f64)
        .collect();

    if !strong_features.is_empty() {
        log_detail!("   ★ High discriminative power (consider increasing weight):");
        for f in &strong_features {
            log_detail!("     → {}: {:.4}", f.feature_name, f.discriminative_power);
        }
    }
    if !weak_features.is_empty() {
        log_detail!("   · Low discriminative power (consider reducing weight or removing):");
        for f in &weak_features {
            log_detail!("     → {}: {:.4}", f.feature_name, f.discriminative_power);
        }
    }

    log_detail!("");
    Ok(())
}

/// Prints the blind spots discovered in the inference logs.
///
/// # Errors
/// Returns an error if the database query fails.
fn print_blind_spots(conn: &mut postgres::Client) -> Result<()> {
    log_section!(foundation::infra::static_logs::messages::LABEL_BLIND_SPOT_AUDIT);

    let spots = query_inference_blind_spots(conn, 0.6)?;

    if spots.is_empty() {
        log_info!(
            foundation::infra::static_logs::messages::LABEL_SESSION_SUMMARY,
            "{} No blind spots detected (all regions have confidence ≥ 0.6).",
            pick(symbols::SUCCESS, symbols::plain::SUCCESS),
        );
        log_detail!("      This may also mean insufficient inference log data.");
        log_detail!("");
        return Ok(());
    }

    log_detail!(
        "   {:<12} {:<12} {:>10} {:>10} {:>8} Typical Exit Path",
        "Duration(s)",
        "WebP Ratio",
        "Avg Conf.",
        "Avg Final",
        "Samples"
    );

    for spot in &spots {
        let layer = media_conversion_gate::delivery_db_diag_cell_or_unknown(
            spot.example_layer_exit.as_deref(),
        );
        let avg_final = media_conversion_gate::ui_f64_or_na(
            spot.avg_final_probability,
            "db_diagnostics_avg_final_probability",
            3,
        );
        log_detail!(
            "   {:<12.0} {:<12.0} {:>10.3} {:>10} {:>8} {}",
            spot.duration_bucket,
            spot.webp_bucket,
            spot.avg_knn_confidence,
            avg_final,
            spot.sample_count,
            layer
        );
    }

    log_detail!("");
    log_info!(
        foundation::infra::static_logs::messages::LABEL_STRATEGY,
        "{} These regions need more training samples. Prioritize collecting",
        pick(symbols::INFO, symbols::plain::INFO),
    );
    log_detail!("      content matching the duration/WebP ratio ranges listed above.");
    log_detail!("");
    Ok(())
}

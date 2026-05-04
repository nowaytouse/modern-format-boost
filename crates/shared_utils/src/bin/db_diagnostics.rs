//! Database Diagnostics CLI
//!
//! Prints inference log summary, feature discriminative power analysis,
//! and blind spot reports to help calibrate the decision tree.
//!
//! Usage: cargo run --bin `db_diagnostics`

use anyhow::Result;
use shared_utils::database::{
    init_schema, open_pg_client, query_feature_discriminative_power, query_inference_blind_spots,
    query_inference_log_summary,
};
use shared_utils::Rational;

fn main() -> Result<()> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           Database Feedback Loop Diagnostics                ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    let mut conn = open_pg_client()?;
    init_schema(&mut conn)?;

    // ── Section 1: Inference Log Summary ──
    print_inference_summary(&mut conn)?;

    // ── Section 2: Feature Discriminative Power (Level 1) ──
    print_discriminative_power(&mut conn)?;

    // ── Section 3: Blind Spot Discovery (Level 3) ──
    print_blind_spots(&mut conn)?;

    println!();
    println!("═══════════════════════════════════════════════════════════════");
    println!("💡 Tip: Run assessments on more files to accumulate inference logs.");
    println!("   Once enough data is collected, blind spots and weight recommendations");
    println!("   will become increasingly reliable.");

    Ok(())
}

/// Prints a summary of the inference logs.
///
/// # Errors
/// Returns an error if the database query fails.
fn print_inference_summary(conn: &mut postgres::Client) -> Result<()> {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  📊 Inference Log Summary (Level 4)                        │");
    println!("└─────────────────────────────────────────────────────────────┘");

    let summary = query_inference_log_summary(conn)?;

    if summary.total_records == 0 {
        println!("   ⚠️  No inference records yet. Run assess_loop_intent_from_meta");
        println!("      on some files to start accumulating data.");
        println!();
        return Ok(());
    }

    println!("   Total inferences logged: {}", summary.total_records);
    let permille = {
        let ratio = Rational::from(summary.layer7_fallback_count)
            / Rational::from(summary.total_records.max(1));
        let res = ratio * Rational::from(10_000);
        res.to_f64()
    };
    println!(
        "   Layer 7 fallbacks:       {} ({:.1}%)",
        summary.layer7_fallback_count,
        permille / 100.0
    );

    if let Some(avg_tree) = summary.avg_tree_probability {
        println!("   Avg tree probability:    {avg_tree:.3}");
    }
    if let Some(avg_knn) = summary.avg_knn_confidence {
        println!("   Avg KNN confidence:      {avg_knn:.3}");
    }
    if let Some(avg_final) = summary.avg_final_probability {
        println!("   Avg final probability:   {avg_final:.3}");
    }

    println!();
    println!("   Verdict Distribution:");
    for (verdict, count) in &summary.verdict_counts {
        let ratio = Rational::from(*count) / Rational::from(summary.total_records.max(1));
        let pct = ratio.to_f64() * 100.0;
        let bar = "█".repeat(shared_utils::numeric_cast::f64_to_usize_sat(pct / 5.0));
        println!("     {verdict:<14} {count:>5} ({pct:>5.1}%) {bar}");
    }

    println!();
    println!("   Layer Exit Distribution:");
    for (layer, count) in &summary.layer_exit_counts {
        let ratio = Rational::from(*count) / Rational::from(summary.total_records.max(1));
        let pct = ratio.to_f64() * 100.0;
        println!("     {layer:<40} {count:>5} ({pct:>5.1}%)");
    }

    println!();
    Ok(())
}

/// Prints the discriminative power of each feature.
///
/// # Errors
/// Returns an error if the database query fails.
fn print_discriminative_power(conn: &mut postgres::Client) -> Result<()> {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  🔬 Feature Discriminative Power (Level 1)                 │");
    println!("└─────────────────────────────────────────────────────────────┘");

    let features = query_feature_discriminative_power(conn)?;

    if features.is_empty() {
        println!("   ⚠️  Not enough labeled samples to compute discriminative power.");
        println!("      Need both 'high' (loop) and 'video' (non-loop) labeled samples.");
        println!();
        return Ok(());
    }

    println!(
        "   {:<28} {:>12} {:>12} {:>12} {:>6}",
        "Feature", "Mean(Loop)", "Mean(Video)", "Discrim.", "N"
    );
    println!("   {}", "─".repeat(74));

    for f in &features {
        let strong_str = f
            .mean_loop_strong
            .map_or_else(|| "   N/A".to_string(), |v| format!("{v:>12.4}"));
        let weak_str = f
            .mean_loop_weak
            .map_or_else(|| "   N/A".to_string(), |v| format!("{v:>12.4}"));

        let indicator = if f.discriminative_power.abs() > 0.5 {
            "★"
        } else if f.discriminative_power.abs() > 0.2 {
            "○"
        } else {
            "·"
        };

        println!(
            " {indicator} {:<28} {strong_str} {weak_str} {:>12.4} {:>6}",
            f.feature_name, f.discriminative_power, f.sample_count
        );
    }

    // Print actionable recommendations
    println!();
    let strong_features: Vec<_> = features
        .iter()
        .filter(|f| f.discriminative_power.abs() > 0.5)
        .collect();
    let weak_features: Vec<_> = features
        .iter()
        .filter(|f| f.discriminative_power.abs() < 0.1)
        .collect();

    if !strong_features.is_empty() {
        println!("   ★ High discriminative power (consider increasing weight):");
        for f in &strong_features {
            println!("     → {}: {:.4}", f.feature_name, f.discriminative_power);
        }
    }
    if !weak_features.is_empty() {
        println!("   · Low discriminative power (consider reducing weight or removing):");
        for f in &weak_features {
            println!("     → {}: {:.4}", f.feature_name, f.discriminative_power);
        }
    }

    println!();
    Ok(())
}

/// Prints the blind spots discovered in the inference logs.
///
/// # Errors
/// Returns an error if the database query fails.
fn print_blind_spots(conn: &mut postgres::Client) -> Result<()> {
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│  🔍 Blind Spot Discovery (Level 3)                        │");
    println!("└─────────────────────────────────────────────────────────────┘");

    let spots = query_inference_blind_spots(conn, 0.6)?;

    if spots.is_empty() {
        println!("   ✅ No blind spots detected (all regions have confidence ≥ 0.6).");
        println!("      This may also mean insufficient inference log data.");
        println!();
        return Ok(());
    }

    println!(
        "   {:<12} {:<12} {:>10} {:>10} {:>8} Typical Layer",
        "Duration(s)", "WebP Ratio", "Avg Conf.", "Avg Final", "Count"
    );
    println!("   {}", "─".repeat(70));

    for spot in &spots {
        let layer = spot.example_layer_exit.as_deref().unwrap_or("?");
        let avg_final = spot
            .avg_final_probability
            .map_or_else(|| "N/A".to_string(), |v| format!("{v:.3}"));
        println!(
            "   {:<12.0} {:<12.0} {:>10.3} {:>10} {:>8} {}",
            spot.duration_bucket,
            spot.webp_bucket,
            spot.avg_knn_confidence,
            avg_final,
            spot.sample_count,
            layer
        );
    }

    println!();
    println!("   💡 These regions need more training samples. Prioritize collecting");
    println!("      content matching the duration/WebP ratio ranges listed above.");
    println!();

    Ok(())
}

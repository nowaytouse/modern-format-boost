// ============================================================================
// 📋 XMP Metadata Merger CLI
// ============================================================================
//
// Reliable XMP sidecar file merger with multiple matching strategies.
//
// Usage:
//   xmp-merge /path/to/media
//   xmp-merge --delete-xmp /path/to/media
//   xmp-merge --verbose /path/to/media
//
// ============================================================================

use anyhow::{Context, Result};
use clap::Parser;
use console::{style, Term};
use indicatif::{ProgressBar, ProgressStyle};
use shared_utils::{XmpMerger, XmpMergerConfig, MergeSummary};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "xmp-merge")]
#[command(author = "Pixly Team")]
#[command(version = "1.0.0")]
#[command(about = "Reliable XMP sidecar metadata merger", long_about = None)]
struct Args {
    /// Target directory containing XMP files
    #[arg(required = true)]
    directory: PathBuf,

    /// Delete XMP files after successful merge
    #[arg(long, short = 'd')]
    delete_xmp: bool,

    /// Show verbose output (debug matching strategies)
    #[arg(long, short = 'v')]
    verbose: bool,

    /// Keep original files (don't use -overwrite_original)
    #[arg(long)]
    keep_backup: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let term = Term::stdout();

    // Print header
    term.write_line("╔══════════════════════════════════════════════╗")?;
    term.write_line("║   📋 XMP Metadata Merger v1.0 (Rust)         ║")?;
    term.write_line("╚══════════════════════════════════════════════╝")?;
    term.write_line("")?;

    // Validate directory
    if !args.directory.exists() {
        anyhow::bail!("Directory does not exist: {}", args.directory.display());
    }
    if !args.directory.is_dir() {
        anyhow::bail!("Path is not a directory: {}", args.directory.display());
    }

    // Check exiftool
    XmpMerger::check_exiftool().context("ExifTool dependency check failed")?;

    // Configure merger
    let config = XmpMergerConfig {
        delete_xmp_after_merge: args.delete_xmp,
        overwrite_original: !args.keep_backup,
        preserve_timestamps: true,
        verbose: args.verbose,
    };

    let merger = XmpMerger::new(config.clone());

    // Print configuration
    println!("📁 Target: {}", style(args.directory.display()).cyan());
    if config.delete_xmp_after_merge {
        println!("🗑️  Mode: {}", style("Delete XMP after merge").yellow());
    }
    if config.verbose {
        println!("🔍 Verbose: {}", style("Enabled").green());
    }
    println!();

    // Find XMP files
    println!("📊 Scanning for XMP files...");
    let xmp_files = merger.find_xmp_files(&args.directory)?;
    
    if xmp_files.is_empty() {
        println!("{}", style("No XMP files found.").yellow());
        return Ok(());
    }

    println!("📁 Found: {} XMP files", style(xmp_files.len()).green());
    println!();

    // Create progress bar
    let pb = ProgressBar::new(xmp_files.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
            .unwrap()
            .progress_chars("█▓░"),
    );

    // Process files
    let mut results = Vec::with_capacity(xmp_files.len());
    
    for xmp_path in &xmp_files {
        let result = merger.process_xmp(xmp_path);
        
        // Print result
        let filename = xmp_path.file_name().unwrap_or_default().to_string_lossy();
        
        if result.success {
            if let Some(ref media) = result.media_path {
                let media_name = media.file_name().unwrap_or_default().to_string_lossy();
                let strategy = result.match_strategy.as_deref().unwrap_or("unknown");
                pb.println(format!(
                    "  {} {} → {} [{}]",
                    style("✅").green(),
                    filename,
                    style(&media_name).cyan(),
                    style(strategy).dim()
                ));
            }
        } else if result.media_path.is_none() {
            pb.println(format!(
                "  {} {} ({})",
                style("⏭️").yellow(),
                filename,
                style("no matching media").dim()
            ));
        } else {
            pb.println(format!(
                "  {} {} ({})",
                style("❌").red(),
                filename,
                style(&result.message).dim()
            ));
        }
        
        results.push(result);
        pb.inc(1);
    }

    pb.finish_and_clear();

    // Print summary
    let summary = MergeSummary::from_results(&results);
    
    println!();
    println!("╔══════════════════════════════════════════════╗");
    println!("║   📊 Merge Complete                          ║");
    println!("╠══════════════════════════════════════════════╣");
    println!("║  ✅ Successful:    {:>20}       ║", style(summary.success).green());
    println!("║  ❌ Failed:        {:>20}       ║", style(summary.failed).red());
    println!("║  ⏭️  Skipped:       {:>20}       ║", style(summary.skipped).yellow());
    println!("╠══════════════════════════════════════════════╣");
    println!("║  📈 Match Strategies:                        ║");
    
    for (strategy, count) in &summary.strategies {
        let strategy_name = match strategy.as_str() {
            "direct_match" => "Direct (.jpg.xmp → .jpg)",
            "same_name" => "Same name (.xmp → .jpg)",
            "xmp_metadata" => "XMP metadata extraction",
            "document_id" => "DocumentID matching",
            "no_match" => "No match found",
            _ => strategy,
        };
        println!("║    • {:<25} {:>5}       ║", strategy_name, count);
    }
    
    println!("╚══════════════════════════════════════════════╝");

    if summary.failed > 0 {
        std::process::exit(1);
    }

    Ok(())
}

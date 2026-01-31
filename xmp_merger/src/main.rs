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
use shared_utils::checkpoint::CheckpointManager;
use shared_utils::{MergeSummary, XmpMerger, XmpMergerConfig};
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

    /// Start fresh (ignore previous progress)
    #[arg(long)]
    fresh: bool,
}

fn main() -> Result<()> {
    // 🔥 v7.8: 初始化日志系统
    let _ = shared_utils::logging::init_logging(
        "xmp_merger",
        shared_utils::logging::LogConfig::default(),
    );

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

    // Initialize checkpoint manager for resume support
    let mut checkpoint = CheckpointManager::new(&args.directory)
        .context("Failed to initialize checkpoint manager")?;

    // Check for existing lock
    if let Some(pid) = checkpoint.check_lock()? {
        eprintln!(
            "⚠️  Another process (PID {}) is already processing this directory",
            pid
        );
        eprintln!(
            "   If this is incorrect, delete: {}",
            checkpoint.progress_dir().join("processing.lock").display()
        );
        std::process::exit(1);
    }

    // Clear progress if --fresh flag
    if args.fresh {
        checkpoint.clear_progress()?;
        println!("🔄 Starting fresh (cleared previous progress)");
    }

    // Show resume info
    if checkpoint.is_resume_mode() {
        println!(
            "🔄 Resuming: {} files already completed",
            style(checkpoint.completed_count()).green()
        );
    }

    // Acquire lock
    checkpoint.acquire_lock()?;

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
        checkpoint.cleanup()?;
        return Ok(());
    }

    // Filter out already completed files
    let pending_files: Vec<_> = xmp_files
        .iter()
        .filter(|f| !checkpoint.is_completed(f))
        .collect();

    let skipped_count = xmp_files.len() - pending_files.len();

    println!("📁 Found: {} XMP files", style(xmp_files.len()).green());
    if skipped_count > 0 {
        println!(
            "⏭️  Skipping: {} already processed",
            style(skipped_count).yellow()
        );
    }
    println!();

    // Handle case where all files already processed
    if pending_files.is_empty() {
        println!("{}", style("All files already processed!").green());
        checkpoint.cleanup()?;
        return Ok(());
    }

    // Create progress bar - 🔥 v5.30: 统一进度条样式
    let pb = ProgressBar::new(pending_files.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            // 🔥 v7.9.1: 使用 {eta} 替代 {eta_precise}，避免溢出
            .template("{spinner:.green} {prefix:.cyan.bold} ▕{bar:35.green/black}▏ {percent:>3}% • {pos}/{len} • ⏱️ {elapsed_precise} (ETA: {eta}) • {msg}")
            .unwrap()
            .progress_chars("██▓░")
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
    );
    pb.set_prefix("XMP Merge");

    // Process files with checkpoint tracking
    let mut results = Vec::with_capacity(pending_files.len());

    for xmp_path in &pending_files {
        let result = merger.process_xmp(xmp_path);

        // Print result
        let filename = xmp_path.file_name().unwrap_or_default().to_string_lossy();

        if result.success {
            // Mark as completed ONLY on success
            checkpoint.mark_completed(xmp_path)?;

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
            // No matching media - also mark as "completed" to skip on resume
            // (no point retrying if media doesn't exist)
            checkpoint.mark_completed(xmp_path)?;

            pb.println(format!(
                "  {} {} ({})",
                style("⏭️").yellow(),
                filename,
                style("no matching media").dim()
            ));
        } else {
            // Actual failure - DON'T mark as completed, will retry on resume
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
    println!(
        "║  ✅ Successful:    {:>20}       ║",
        style(summary.success).green()
    );
    println!(
        "║  ❌ Failed:        {:>20}       ║",
        style(summary.failed).red()
    );
    println!(
        "║  ⏭️  Skipped:       {:>20}       ║",
        style(summary.skipped).yellow()
    );
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

    // Cleanup checkpoint on success (no failures that need retry)
    if summary.failed == 0 {
        checkpoint.cleanup()?;
    } else {
        // Keep progress for resume
        checkpoint.release_lock()?;
        println!();
        println!(
            "💡 {} failures - run again to retry failed files",
            summary.failed
        );
        std::process::exit(1);
    }

    Ok(())
}

//! Rust XMP sidecar merge utility.
//!
//! Scans a file or directory for `.xmp` sidecars, pairs each sidecar with an
//! adjacent media file using an 8-strategy pipeline and delegates
//! metadata writing to `exiftool`.
//!
//! Reuses the core library implementation from `foundation::xmp_merger`.

use anyhow::{Result, bail};
use clap::Parser;
use dev::infra::ui_tokens::pick_symbol;
use foundation::xmp_merger::{Config as XmpMergerConfig, LogLevel, OverwriteMode, XmpMerger};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "merge_xmp",
    about = "Merge adjacent XMP sidecars into media files (8-strategy pipeline, timestamp-safe)"
)]
struct Args {
    #[arg(value_name = "FILE_OR_DIR")]
    target: PathBuf,

    /// Delete sidecar after verified merge (default: delete on success,
    /// matching py behaviour)
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    delete_sidecar: bool,

    /// Print planned merges without writing metadata
    #[arg(long)]
    dry_run: bool,

    /// Verbose output
    #[arg(long, short = 'v')]
    verbose: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct MergeSummary {
    merged: usize,
    skipped: usize,
    failed: usize,
}

// ── top-level ────────────────────────────────────────────────────────────────

fn merge_target(target: &Path, args: &Args) -> Result<MergeSummary> {
    let config = XmpMergerConfig {
        delete_xmp_after_merge: false, // Handle delete manually to mirror py/bin output/dry_run
        overwrite_mode: OverwriteMode::Original,
        preserve_timestamps: true,
        log_level: if args.verbose {
            LogLevel::Verbose
        } else {
            LogLevel::Quiet
        },
    };
    let merger = XmpMerger::new(config);

    let xmps = if target.is_file() {
        if !target
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("xmp"))
        {
            bail!("Target file is not an XMP file: {}", target.display());
        }
        vec![target.to_path_buf()]
    } else {
        merger.find_xmp_files(target)?
    };

    if xmps.is_empty() {
        println!("No .xmp files found in target.");
        return Ok(MergeSummary::default());
    }

    println!(
        "Found {} XMP file(s). Running 8-strategy pipeline...\n",
        xmps.len()
    );

    let mut summary = MergeSummary::default();
    for xmp in &xmps {
        let (media_path, strategy) = match merger.find_media_file(xmp) {
            Ok(res) => res,
            Err(err) => {
                summary.failed += 1;
                eprintln!(
                    "  {} Error matching {}: {err}",
                    pick_symbol("❌", "[ERROR]"),
                    xmp.display()
                );
                continue;
            }
        };
        let sidecar_name = xmp.file_name().unwrap_or_default().to_string_lossy();

        if let Some(media) = media_path {
            let media_name = media.file_name().unwrap_or_default().to_string_lossy();

            if args.verbose {
                println!(
                    "  {} Merge [{}]: {} → {}",
                    pick_symbol("🔗", "[MERGE]"),
                    strategy,
                    sidecar_name,
                    media_name
                );
            } else {
                println!("  Merge [{}]: {} → {}", strategy, sidecar_name, media_name);
            }

            if !args.dry_run {
                match merger.merge_xmp(xmp, &media) {
                    Ok(()) => {
                        println!("  {} Success (XMP merged)", pick_symbol("✅", "[OK]"));
                        if args.delete_sidecar {
                            let _ = fs::remove_file(xmp);
                        }
                        summary.merged += 1;
                    }
                    Err(err) => {
                        summary.failed += 1;
                        eprintln!(
                            "  {} Failed {}: {err}",
                            pick_symbol("❌", "[ERROR]"),
                            xmp.display()
                        );
                    }
                }
            } else {
                summary.merged += 1;
            }
        } else {
            summary.skipped += 1;
            println!(
                "  {} Skipped (no match): {}",
                pick_symbol("⚠️", "[WARN]"),
                sidecar_name
            );
        }
    }
    Ok(summary)
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Resolve through the shared policy so GUI launches and tool overrides agree.
    let Some(exiftool) = foundation::common_utils::resolve_tool_path("exiftool") else {
        bail!("exiftool was not found or failed its runtime health check");
    };
    if Command::new(exiftool).arg("-ver").output().is_err() {
        bail!("exiftool could not be started");
    }

    println!();
    println!("Modern Format Boost — XMP Merger Tool (8-Strategy Edition)");
    println!("Target: {}", args.target.display());
    println!();

    let summary = merge_target(&args.target, &args)?;

    println!("\nSummary:");
    println!("  merged : {}", summary.merged);
    println!("  skipped: {}", summary.skipped);
    println!("  failed : {}", summary.failed);

    if summary.failed > 0 || summary.skipped > 0 {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_target_dry_run_success() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let xmp_path = temp.path().join("img.jpg.xmp");
        let media_path = temp.path().join("img.jpg");

        fs::write(&xmp_path, b"xmp_content")?;
        fs::write(&media_path, b"jpeg_content")?;

        let args = Args {
            target: temp.path().to_path_buf(),
            delete_sidecar: true,
            dry_run: true,
            verbose: true,
        };

        let summary = merge_target(temp.path(), &args)?;
        assert_eq!(summary.merged, 1);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.failed, 0);

        assert!(xmp_path.exists());
        Ok(())
    }

    #[test]
    fn test_merge_target_no_match() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let xmp_path = temp.path().join("img.jpg.xmp");

        fs::write(&xmp_path, b"xmp_content")?;

        let args = Args {
            target: temp.path().to_path_buf(),
            delete_sidecar: true,
            dry_run: false,
            verbose: false,
        };

        let summary = merge_target(temp.path(), &args)?;
        assert_eq!(summary.merged, 0);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.failed, 0);
        Ok(())
    }
}

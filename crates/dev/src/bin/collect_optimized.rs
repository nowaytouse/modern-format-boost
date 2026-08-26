//! Collect backup originals for JXL files identified by the recovery audit.
//!
//! This binary has one purpose: re-probe the live JXL set, resolve exact
//! originals from a folder/file or Photos backup, and copy them into a
//! resumable BLAKE3-verified recovery tree. It never reorganizes or moves the
//! audited source or backup.

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::hardening::{flush_stdout, read_stdin_line};
use std::path::PathBuf;

const FAILURE_PREVIEW: usize = 10;

#[derive(Parser, Debug)]
#[command(
    name = "collect_optimized",
    about = "Collect audited JXL recovery originals from an exact backup"
)]
struct Args {
    /// Audited JXL file, folder, or Photos library
    source: PathBuf,

    /// Destination directory for recovered originals and proof manifest
    destination: PathBuf,

    /// Exact backup file, folder, or Photos library
    #[arg(long, value_name = "PATH")]
    backup: PathBuf,

    /// Preview exact matches without copying files
    #[arg(long)]
    dry_run: bool,

    /// Compare two Photos libraries read-only instead of collecting originals
    #[arg(long, conflicts_with = "dry_run")]
    compare: bool,

    /// Skip the interactive confirmation prompt
    #[arg(long)]
    yes: bool,
}

fn main() -> Result<()> {
    foundation::init_ghost_mode().context("initialize ghost mode")?;
    let args = Args::parse();
    let source = args
        .source
        .canonicalize()
        .with_context(|| format!("resolve audited source: {}", args.source.display()))?;
    let backup = args
        .backup
        .canonicalize()
        .with_context(|| format!("resolve backup source: {}", args.backup.display()))?;
    let destination = if args.destination.exists() {
        args.destination
            .canonicalize()
            .with_context(|| format!("resolve destination: {}", args.destination.display()))?
    } else {
        args.destination
    };

    if args.compare {
        let summary = dev::infra::recovery_collection::run_recovery_comparison(
            &source,
            &backup,
            &destination,
        )?;
        println!("\nBackup comparison summary");
        println!("  matched identities:  {}", summary.matched);
        println!("  source only:         {}", summary.source_only);
        println!("  backup only:         {}", summary.backup_only);
        println!("  different payloads:  {}", summary.different);
        println!("  needs review:        {}", summary.needs_review);
        if let Some(report) = summary.report {
            println!("  comparison report:   {}", report.display());
        }
        return Ok(());
    }

    if !args.dry_run && !args.yes {
        print!(
            "Collect only live non-reconstructible JXL originals from {} into {}? [y/N]: ",
            backup.display(),
            destination.display()
        );
        flush_stdout();
        let mut answer = String::new();
        read_stdin_line(&mut answer);
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Cancelled. No files were copied.");
            return Ok(());
        }
    }

    let summary = dev::infra::recovery_collection::run_recovery_collection(
        &source,
        &backup,
        &destination,
        args.dry_run,
    )?;
    println!("\nRecovery collection summary");
    println!("  affected JXL assets: {}", summary.selected);
    println!("  copied files:        {}", summary.copied);
    println!("  already verified:    {}", summary.skipped);
    println!("  needs review:        {}", summary.needs_review);
    println!("  failures:            {}", summary.failed.len());
    if let Some(manifest) = &summary.manifest {
        println!("  proof manifest:      {}", manifest.display());
    }
    for failure in summary.failed.iter().take(FAILURE_PREVIEW) {
        eprintln!("  [FAIL] {failure}");
    }
    if !summary.succeeded() {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_is_mandatory() {
        assert!(Args::try_parse_from(["collect_optimized", "source", "destination"]).is_err());
        assert!(
            Args::try_parse_from([
                "collect_optimized",
                "source",
                "destination",
                "--backup",
                "backup",
            ])
            .is_ok()
        );
        assert!(
            Args::try_parse_from([
                "collect_optimized",
                "source",
                "destination",
                "--backup",
                "backup",
                "--compare",
            ])
            .is_ok()
        );
        assert!(
            Args::try_parse_from([
                "collect_optimized",
                "source",
                "destination",
                "--backup",
                "backup",
                "--compare",
                "--dry-run",
            ])
            .is_err()
        );
    }
}

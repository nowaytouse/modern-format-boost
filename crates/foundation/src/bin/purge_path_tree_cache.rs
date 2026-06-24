//! Purge path-tree scan snapshots (`PostgreSQL` + `SQLite` replica) — SSOT for
//! `cache_cleaner`.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Purge path_tree_snapshots (PG) and path_tree blob namespace (SQLite)")]
struct Args {
    /// Remove snapshots whose `root_path` equals or is under this directory.
    #[arg(long, conflicts_with = "all")]
    under: Option<PathBuf>,
    /// Remove all path-tree snapshots.
    #[arg(long, conflicts_with = "under")]
    all: bool,
}

fn main() -> Result<()> {
    foundation::entry_guard::assert_dev_tool_entry("purge_path_tree_cache")
        .context("purge_path_tree_cache entry guard")?;
    let args = Args::parse();
    let deleted = if args.all {
        foundation::path_tree_cache::purge_all_path_tree_snapshots()
    } else if let Some(path) = args.under {
        foundation::path_tree_cache::purge_path_tree_under(&path)
    } else {
        anyhow::bail!("specify --under PATH or --all");
    }?;
    // Last stdout line = deleted row count (parsed by cache_cleaner.rs).
    println!("{deleted}");
    Ok(())
}

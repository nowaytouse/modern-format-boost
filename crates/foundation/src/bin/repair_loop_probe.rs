//! Re-read media files on disk to backfill missing loop probe fields in
//! `loop_samples.metadata`.
//!
//! This can take a long time (one decode per broken row). It is **not** full
//! re-ingest or retrain; use `refresh_stats` / `refresh-loop-stats` afterward
//! for fast SQL-only stats + embedding refresh.
use anyhow::{Context, Result};
use foundation::database::{open_pg_client, repair_loop_samples_missing_probe_fields};
use foundation::modern_ui::symbols;
use foundation::multi_scenario_db::init_multi_scenario_schema;

fn main() -> Result<()> {
    foundation::entry_guard::assert_pipeline_tool_entry("repair_loop_probe")
        .context("repair_loop_probe entry guard")?;
    foundation::progress_mode::configure_terminal_ux(false);
    foundation::ui_stderr::line(
        symbols::INFO,
        symbols::plain::INFO,
        "Repairing loop_samples metadata from on-disk source_path (slow; not full retrain)...",
    );
    let mut conn = open_pg_client()?;
    init_multi_scenario_schema(&mut conn)?;
    let row_count: i64 = conn
        .query_one(
            "SELECT COUNT(*)::bigint FROM loop_samples WHERE frame_count > 1",
            &[],
        )
        .context("loop_samples COUNT for repair progress")?
        .get(0);
    foundation::ui_stderr::line(
        symbols::INFO,
        symbols::plain::INFO,
        format!(
            "loop_samples with frame_count>1: {row_count} (repair targets rows missing probe JSON \
             keys)"
        ),
    );
    let (repaired, skipped_no_path, reprobe_failed) =
        repair_loop_samples_missing_probe_fields(&mut conn)
            .context("loop_samples probe-field repair failed")?;
    foundation::ui_stderr::line(
        symbols::SUCCESS,
        symbols::plain::SUCCESS,
        format!(
            "Loop probe repair done: repaired={repaired} skipped_no_source_path={skipped_no_path} \
             reprobe_failed={reprobe_failed}"
        ),
    );
    Ok(())
}

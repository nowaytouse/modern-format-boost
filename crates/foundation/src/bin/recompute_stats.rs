use anyhow::Result;
use foundation::database::{open_pg_client, refresh_loop_intent_feature_stats};
use foundation::modern_ui::symbols;
use foundation::multi_scenario_db::init_multi_scenario_schema;

fn main() -> Result<()> {
    foundation::entry_guard::assert_pipeline_tool_entry("recompute_stats")?;
    foundation::progress_mode::configure_terminal_ux(false);
    foundation::ui_stderr::line(
        symbols::ROCKET,
        symbols::plain::ROCKET,
        "Starting LoopIntent feature-stat refresh...",
    );
    let mut conn = open_pg_client()?;
    init_multi_scenario_schema(&mut conn)?;
    refresh_loop_intent_feature_stats(&mut conn)?;
    foundation::ui_stderr::line(
        symbols::SUCCESS,
        symbols::plain::SUCCESS,
        "LoopIntent feature statistics successfully recomputed.",
    );
    Ok(())
}

//! Maintenance utility to refresh the global collection statistics.
use anyhow::{Context, Result};
use foundation::database::{open_pg_client, refresh_loop_intent_feature_stats};
use foundation::multi_scenario_db::init_multi_scenario_schema;

fn main() -> Result<()> {
    foundation::entry_guard::assert_pipeline_tool_entry("refresh_stats")
        .context("refresh_stats entry guard")?;
    let mut conn = open_pg_client()?;
    init_multi_scenario_schema(&mut conn)?;
    refresh_loop_intent_feature_stats(&mut conn)?;
    Ok(())
}

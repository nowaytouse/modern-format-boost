//! Maintenance utility to refresh the global collection statistics.
use anyhow::Result;
use shared_utils::database::{open_pg_client, refresh_feature_stats};

fn main() -> Result<()> {
    let mut conn = open_pg_client()?;
    refresh_feature_stats(&mut conn)?;
    Ok(())
}

use anyhow::Result;
use shared_utils::database::{open_pg_client, refresh_feature_stats};

fn main() -> Result<()> {
    let mut conn = open_pg_client()?;
    println!("🔄 Refreshing feature statistics...");
    refresh_feature_stats(&mut conn)?;
    println!("✅ Feature statistics refreshed successfully!");
    Ok(())
}

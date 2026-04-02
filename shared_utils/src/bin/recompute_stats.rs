use anyhow::Result;
use shared_utils::gif_value_db::{open_pg_client, refresh_feature_stats};

fn main() -> Result<()> {
    println!("🚀 Starting KNN Database Retraining (Feature Stats Refresh)...");
    let mut conn = open_pg_client()?;
    refresh_feature_stats(&mut conn)?;
    println!("✅ KNN Feature Statistics successfully recomputed for GIF-only dataset.");
    Ok(())
}

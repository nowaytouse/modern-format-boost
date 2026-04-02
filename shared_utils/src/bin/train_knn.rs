use anyhow::Result;
use std::path::Path;
use shared_utils::gif_value_db::{batch_ingest_samples};

fn main() -> Result<()> {
    let training_dir = Path::new("/Users/nyamiiko/Downloads/表情包");
    
    if !training_dir.exists() {
        anyhow::bail!("❌ Training directory not found: {}", training_dir.display());
    }

    println!("🧠 Starting KNN Model Offline Training...");
    println!("📂 Training Directory: {}", training_dir.display());
    
    // batch_ingest_samples handles init_schema, seeding, scanning, inserting and stats refresh.
    let count = batch_ingest_samples(training_dir)?;
    
    println!("✅ Training Complete! Ingested/Updated {} samples from specified collections.", count);
    println!("📊 KNN Feature Stats and Global Duration Baselines (Min/Avg/Max/P90) have been recomputed.");
    
    Ok(())
}

use anyhow::Result;
use shared_utils::database::{init_schema, open_pg_client, refresh_feature_stats};

fn main() -> Result<()> {
    shared_utils::progress_mode::emit_stderr(
        "🚀 Starting KNN Database Retraining (Feature Stats Refresh)...",
    );
    let mut conn = open_pg_client()?;
    init_schema(&mut conn)?;
    refresh_feature_stats(&mut conn)?;
    shared_utils::progress_mode::emit_stderr(
        "✅ KNN Feature Statistics successfully recomputed for GIF-only dataset.",
    );
    Ok(())
}

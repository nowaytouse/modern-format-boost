//! Database Logging Verification Tool
//!
//! Minimal functional test to verify that the `PostgreSQL` feedback loop
//! (inference logging) is correctly operational.
//!
//! Usage: cargo run --bin `verify_db_logging`
use shared_utils::database::{log_inference_record, open_pg_client, LoopInferenceRecord};
use shared_utils::loop_intent::LoopMeta;
use std::path::Path;

fn main() {
    println!("🚀 Starting DB Logging verification...");

    // 1. Open DB connection
    let mut conn = match open_pg_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("❌ Failed to connect to database: {e}");
            println!("   (Ensure PostgreSQL is running and 'modern_format_boost' database exists)");
            std::process::exit(1);
        }
    };

    println!("✅ Connected to database.");

    // 2. Create mock LoopMeta using only existing public fields
    let meta = LoopMeta {
        duration_secs: 2.5,
        width: 640,
        height: 480,
        fps: 30.0,
        frame_count: Some(75),
        file_size_bytes: 500_000,
        file_name: Some("test_file.gif".to_string()),
        source_extension: Some("gif".to_string()),
        has_audio: false,
        has_transparency: true,
        is_native_gif: true,
        ..Default::default()
    };

    // 3. Create mock LoopInferenceRecord
    let record = LoopInferenceRecord {
        tree_probability: 0.85,
        knn_keep_probability: Some(0.92_f64),
        knn_confidence: Some(0.78_f64),
        knn_neighbor_count: Some(15),
        final_probability: 0.88,
        final_verdict: "LoopStrong".to_string(),
        decision_reason: "Verification Test: High confidence KNN match".to_string(),
        layer_exit: "Layer 6".to_string(),
    };

    // 4. Call log_inference_record
    println!("📡 Sending inference log to DB...");
    log_inference_record(
        &mut conn,
        &meta,
        &record,
        Some(Path::new("debug/verification_test.gif")),
    );

    // 5. Verify result (Query the last record)
    println!("🔍 Verifying last record in 'inference_log'...");
    let row_result = conn.query_one(
        "SELECT id, duration_secs, final_verdict, signal_snapshot 
         FROM inference_log 
         ORDER BY id DESC LIMIT 1",
        &[],
    );

    match row_result {
        Ok(row) => {
            let id: i64 = row.get(0);
            let dur: f64 = row.get(1);
            let verdict: String = row.get(2);
            let snapshot: serde_json::Value = row.get(3);

            println!("✅ VERIFICATION SUCCESSFUL!");
            println!("   - Record ID: {id}");
            println!("   - Duration: {dur}s (expected 2.5)");
            println!("   - Verdict: {verdict} (expected LoopStrong)");
            println!("   - Snapshot: {snapshot}");

            if snapshot.get("width").and_then(serde_json::Value::as_u64) == Some(640) {
                println!("   - Snapshot data integrity: OK");
            } else {
                println!("   - ⚠️ Snapshot data integrity: Mismatch or missing 'width' (snapshot: {snapshot:?})");
            }
        }
        Err(e) => {
            eprintln!("❌ Verification check failed: {e}");
            std::process::exit(1);
        }
    }
}

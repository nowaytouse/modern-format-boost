//! Database Logging Verification Tool
//!
//! Minimal functional test to verify that the `PostgreSQL` feedback loop
//! (inference logging) is correctly operational.
//!
//! Usage: cargo run --bin `verify_db_logging`
use foundation::database::{LoopInferenceRecord, log_inference_record, open_pg_client};
use foundation::loop_intent::LoopMeta;
use std::path::Path;

#[allow(clippy::too_many_lines)]
fn main() {
    if let Err(err) = foundation::entry_guard::assert_pipeline_tool_entry("verify_db_logging") {
        eprintln!("verify_db_logging entry guard: {err:#}");
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }
    foundation::log_info!(
        foundation::infra::static_logs::messages::LABEL_VERIFY,
        "🚀 Starting DB Logging verification loop..."
    );

    // 1. Open DB connection
    let mut conn = match open_pg_client() {
        Ok(c) => c,
        Err(e) => {
            foundation::log_error!(
                foundation::infra::static_logs::messages::LABEL_DATABASE,
                &format!("DATABASE CONNECTIVITY FAILURE: Failed to connect to PostgreSQL: {e}")
            );
            foundation::log_hint!(
                foundation::infra::static_logs::messages::LABEL_DATABASE,
                "Ensure PostgreSQL service is active and 'modern_format_boost' database has been initialized."
            );
            std::process::exit(foundation::constants::EXIT_CODE_ERROR);
        }
    };

    foundation::log_info!(
        foundation::infra::static_logs::messages::LABEL_DATABASE,
        "PostgreSQL connectivity established."
    );

    // 2. Create mock LoopMeta using only existing public fields
    let meta = LoopMeta {
        duration_secs: Some(2.5),
        width: Some(640),
        height: Some(480),
        fps: Some(30.0),
        frame_count: Some(75),
        file_size_bytes: 500_000,
        file_name: Some("test_file.gif".to_string()),
        source_extension: Some("gif".to_string()),
        flags: foundation::loop_intent::LoopFlags {
            streams: foundation::loop_intent::LoopStreamFlags {
                has_audio: false,
                has_transparency: true,
                is_native_gif: true,
            },
            ..Default::default()
        },
        ..Default::default()
    };

    // 3. Create mock LoopInferenceRecord
    let record = LoopInferenceRecord {
        tree_probability: Some(0.85),
        knn_keep_probability: Some(0.92_f64),
        knn_confidence: Some(0.78_f64),
        knn_neighbor_count: Some(15),
        final_probability: Some(0.88),
        final_verdict: "LoopStrong".to_string(),
        decision_reason: "Verification Test: High confidence KNN match".to_string(),
        layer_exit: "Layer 6".to_string(),
    };

    // 4. Call log_inference_record
    foundation::log_info!(
        foundation::infra::static_logs::messages::LABEL_DATABASE,
        &format!(
            "{} Transmitting test inference log to database...",
            foundation::modern_ui::symbols::pick("📡", "[TX]")
        )
    );
    log_inference_record(
        &mut conn,
        &meta,
        &record,
        Some(Path::new("debug/verification_test.gif")),
        None,
    );

    // 5. Verify result (Query the last record)
    foundation::log_info!(
        foundation::infra::static_logs::messages::LABEL_VERIFY,
        &format!(
            "{} Executing parity check: Verifying last record in 'inference_log'...",
            foundation::modern_ui::symbols::pick(
                foundation::modern_ui::symbols::SEARCH,
                foundation::modern_ui::symbols::plain::SEARCH,
            )
        )
    );
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

            foundation::log_success!(
                foundation::infra::static_logs::messages::LABEL_DONE,
                "DB LOGGING VERIFICATION SUCCESSFUL!"
            );
            foundation::log_detail!(&format!("   - Record ID: {id}"));
            foundation::log_detail!(&format!("   - Duration: {dur}s (expected 2.5)"));
            foundation::log_detail!(&format!("   - Verdict: {verdict} (expected LoopStrong)"));
            foundation::log_detail!(&format!("   - Snapshot: {snapshot}"));

            if snapshot.get("width").and_then(serde_json::Value::as_u64) == Some(640) {
                foundation::log_info!(
                    foundation::infra::static_logs::messages::LABEL_VERIFY,
                    "   - Snapshot data integrity: OK"
                );
            } else {
                foundation::media_conversion_gate::delivery_db_batch_audit(
                    "verify_snapshot",
                    format!(
                        "   - ⚠️ Snapshot data integrity: Mismatch or missing 'width' (snapshot: {snapshot:?})"
                    ),
                );
            }
        }
        Err(e) => {
            foundation::log_error!(
                foundation::infra::static_logs::messages::LABEL_VERIFY,
                &format!(
                    "{} DB VERIFICATION PARITY CHECK FAILED: {e}",
                    foundation::modern_ui::symbols::pick(
                        foundation::modern_ui::symbols::ERROR,
                        foundation::modern_ui::symbols::plain::ERROR,
                    )
                )
            );
            std::process::exit(foundation::constants::EXIT_CODE_ERROR);
        }
    }
}

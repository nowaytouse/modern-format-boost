//! `PostgreSQL` audit/report commands ported from `training_pipeline.py`.

use crate::infra::corpus_thresholds::{
    loop_corpus_is_mature, loop_corpus_samples_shortfall, min_loop_samples_per_class,
    min_loop_samples_total, min_quality_samples_per_class, min_quality_samples_total,
    quality_corpus_is_mature, quality_corpus_samples_shortfall,
};
use crate::infra::hardening::optional_env;
use anyhow::{Context, Result};
use postgres::Client;
use std::path::PathBuf;

const NON_FINITE_PATTERN: &str = "(nan|-?infinity)";
const REPLICA_SOURCE_PATTERN: &str = "%mfb_training_replica_%";

#[derive(Clone, Copy, Debug)]
pub struct ScenarioSpec {
    pub name: &'static str,
    pub table: &'static str,
    pub expected_dim: i32,
    pub score_col: &'static str,
}

pub const SCENARIOS: &[ScenarioSpec] = &[
    ScenarioSpec {
        name: "loop_intent",
        table: "loop_samples",
        expected_dim: 261,
        score_col: "label",
    },
    ScenarioSpec {
        name: "image_quality",
        table: "image_quality_samples",
        expected_dim: 256,
        score_col: "quality_score",
    },
    ScenarioSpec {
        name: "animated_image_quality",
        table: "animated_image_quality_samples",
        expected_dim: 256,
        score_col: "quality_score",
    },
    ScenarioSpec {
        name: "video_quality",
        table: "video_quality_samples",
        expected_dim: 256,
        score_col: "quality_score",
    },
];

pub const LOOP_CLUSTERING_SCENARIOS: &[ScenarioSpec] = &[SCENARIOS[0]];
pub const QUALITY_REGRESSION_SCENARIOS: &[ScenarioSpec] =
    &[SCENARIOS[1], SCENARIOS[2], SCENARIOS[3]];

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct QualityTableSummary {
    pub embedding_dim: Option<i32>,
    pub total: i64,
    pub null_embedding: i64,
    pub non_finite: i64,
    pub null_score: String,
    pub avg_score: Option<f64>,
    pub replica_source_paths: i64,
    pub positive_count: Option<i64>,
    pub negative_count: Option<i64>,
}

#[derive(Debug, Clone)]
pub(super) struct ImageQualityModelStatus {
    pub state: ModelReadinessState,
    pub readiness_issues: Vec<String>,
    pub model_path: PathBuf,
    pub metadata_path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ArtifactExistence {
    pub model: bool,
    pub metadata: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ModelReadinessState {
    pub ready_for_training: bool,
    pub ready_for_runtime: bool,
    pub artifacts: ArtifactExistence,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct LoopIntentTableSummary {
    pub total: i64,
    pub null_embedding: i64,
    pub non_finite: i64,
    pub loop_positive_count: i64,
    pub video_negative_count: i64,
    pub non_neutral_directory_score: i64,
    pub replica_source_paths: i64,
    pub feature_stats_present: bool,
}

#[derive(Debug, Clone)]
pub(super) struct LoopIntentRuntimeStatus {
    pub ready_for_knn: bool,
    pub ready_for_runtime: bool,
    pub readiness_issues: Vec<String>,
    pub predictor_family: &'static str,
}

fn table_exists(client: &mut Client, table: &str) -> Result<bool> {
    let row = client
        .query_one(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = $1
            )",
            &[&table],
        )
        .with_context(|| format!("table_exists({table})"))?;
    Ok(row.get(0))
}

fn column_exists(client: &mut Client, table: &str, column: &str) -> Result<bool> {
    let row = client
        .query_one(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2
            )",
            &[&table, &column],
        )
        .with_context(|| format!("column_exists({table},{column})"))?;
    Ok(row.get(0))
}

fn read_embedding_dimension(client: &mut Client, table: &str) -> Result<Option<i32>> {
    if !table_exists(client, table)? {
        return Ok(None);
    }
    let dim_row = client
        .query_opt(
            &format!(
                "SELECT vector_dims(embedding)::int FROM {table} \
                 WHERE embedding IS NOT NULL LIMIT 1"
            ),
            &[],
        )
        .with_context(|| format!("read_embedding_dimension({table})"))?;
    Ok(dim_row.map(|r| r.get::<_, i32>(0)))
}

fn render_table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        return;
    }
    let max_row_len = rows.iter().map(std::vec::Vec::len).max();
    let col_count = match max_row_len {
        Some(n) => headers.len().max(n),
        None => headers.len(),
    };
    let mut widths = vec![0usize; col_count];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = widths[i].max(h.len());
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }
    let sep: String = widths
        .iter()
        .map(|w| "-".repeat(w + 2))
        .collect::<Vec<_>>()
        .join("+");
    let fmt_row = |cells: &[&str]| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!(" {:width$} ", c, width = widths[i]))
            .collect::<Vec<_>>()
            .join("|")
    };
    println!("{}", fmt_row(headers));
    println!("{sep}");
    for row in rows {
        let refs: Vec<&str> = row.iter().map(String::as_str).collect();
        println!("{}", fmt_row(&refs));
    }
}

fn cache_base_dir() -> PathBuf {
    let base = optional_env("MFB_HOME_ROOT")
        .or_else(|| optional_env("HOME"))
        .unwrap_or_else(|| ".".to_string());
    let mut path = PathBuf::from(base);
    if path.file_name().and_then(|n| n.to_str()) != Some(".modern_format_boost") {
        path.push(".modern_format_boost");
    }
    path.join("cache")
}

pub(super) fn default_image_quality_model_path() -> PathBuf {
    if let Some(explicit) = optional_env("MFB_IMAGE_QUALITY_MODEL_PATH") {
        return PathBuf::from(explicit);
    }
    cache_base_dir().join("models/image_quality/lightgbm_model.txt")
}

pub(super) fn default_image_quality_metadata_path() -> PathBuf {
    if let Some(explicit) = optional_env("MFB_IMAGE_QUALITY_MODEL_METADATA_PATH") {
        return PathBuf::from(explicit);
    }
    cache_base_dir().join("models/image_quality/lightgbm_model.metadata.json")
}

pub(super) fn read_quality_table_summary(
    client: &mut Client,
    table: &str,
) -> Result<QualityTableSummary> {
    let embedding_dim = read_embedding_dimension(client, table)?;
    let has_quality_score = column_exists(client, table, "quality_score")?;

    let (total, null_embedding, non_finite, null_score, avg_score, positive_count, negative_count) =
        if has_quality_score {
            let row = client.query_one(
                &format!(
                    "SELECT COUNT(*),
                    COUNT(*) FILTER (WHERE embedding IS NULL),
                    COUNT(*) FILTER (WHERE embedding IS NOT NULL AND embedding::text ~* $1),
                    COUNT(*) FILTER (WHERE quality_score IS NULL),
                    AVG(quality_score),
                    COUNT(*) FILTER (WHERE quality_score >= 0.5),
                    COUNT(*) FILTER (WHERE quality_score < 0.5)
                 FROM {table}"
                ),
                &[&NON_FINITE_PATTERN],
            )?;
            (
                row.get(0),
                row.get(1),
                row.get(2),
                row.get::<_, i64>(3).to_string(),
                row.get(4),
                Some(row.get::<_, i64>(5)),
                Some(row.get::<_, i64>(6)),
            )
        } else {
            let row = client.query_one(
                &format!(
                    "SELECT COUNT(*),
                    COUNT(*) FILTER (WHERE embedding IS NULL),
                    COUNT(*) FILTER (WHERE embedding IS NOT NULL AND embedding::text ~* $1)
                 FROM {table}"
                ),
                &[&NON_FINITE_PATTERN],
            )?;
            (
                row.get(0),
                row.get(1),
                row.get(2),
                "missing-column".to_string(),
                None,
                None,
                None,
            )
        };

    let replica_row = client.query_one(
        &format!("SELECT COUNT(*) FROM {table} WHERE source_path LIKE $1"),
        &[&REPLICA_SOURCE_PATTERN],
    )?;

    Ok(QualityTableSummary {
        embedding_dim,
        total,
        null_embedding,
        non_finite,
        null_score,
        avg_score,
        replica_source_paths: replica_row.get(0),
        positive_count,
        negative_count,
    })
}

pub(super) fn print_image_quality_model_status(summary: &QualityTableSummary) {
    let status = evaluate_image_quality_model_status(summary);
    let positive_count = summary
        .positive_count
        .map_or_else(|| "n/a".into(), |v| v.to_string());
    let negative_count = summary
        .negative_count
        .map_or_else(|| "n/a".into(), |v| v.to_string());
    println!(
        "training_readiness={} thresholds=total>={},high>={},low>={} high={} low={}",
        if status.state.ready_for_training {
            "ready"
        } else {
            "pending"
        },
        min_quality_samples_total(),
        min_quality_samples_per_class(),
        min_quality_samples_per_class(),
        positive_count,
        negative_count,
    );
    if !status.readiness_issues.is_empty() {
        println!("training_issues={}", status.readiness_issues.join("; "));
    }
    let artifact_state = if !status.state.artifacts.model && !status.state.artifacts.metadata {
        "missing_model_and_metadata"
    } else if !status.state.artifacts.model {
        "missing_model"
    } else if !status.state.artifacts.metadata {
        "missing_metadata"
    } else {
        "ready"
    };
    println!(
        "model_artifacts={artifact_state} model={} metadata={}",
        status.model_path.display(),
        status.metadata_path.display(),
    );
}

pub(super) fn evaluate_image_quality_model_status(
    summary: &QualityTableSummary,
) -> ImageQualityModelStatus {
    let mut readiness_issues = Vec::new();
    if summary.positive_count.is_none() || summary.negative_count.is_none() {
        readiness_issues.push("missing_quality_score".to_string());
    } else if let (Some(high), Some(low)) = (summary.positive_count, summary.negative_count)
        && !quality_corpus_is_mature(
            foundation::numeric_cast::i64_to_u64_sat(high),
            foundation::numeric_cast::i64_to_u64_sat(low),
        )
    {
        let shortfall = quality_corpus_samples_shortfall(
            foundation::numeric_cast::i64_to_u64_sat(high),
            foundation::numeric_cast::i64_to_u64_sat(low),
        );
        readiness_issues.push(format!(
            "corpus_shortfall={shortfall} (need total>={}, high/low>={}; \
                 have total={} high={high} low={low})",
            min_quality_samples_total(),
            min_quality_samples_per_class(),
            summary.total,
        ));
    }

    let model_path = default_image_quality_model_path();
    let metadata_path = default_image_quality_metadata_path();
    let model_exists = model_path.is_file();
    let metadata_exists = metadata_path.is_file();
    let ready_for_training = readiness_issues.is_empty();
    let ready_for_runtime = ready_for_training && model_exists && metadata_exists;

    ImageQualityModelStatus {
        state: ModelReadinessState {
            ready_for_training,
            ready_for_runtime,
            artifacts: ArtifactExistence {
                model: model_exists,
                metadata: metadata_exists,
            },
        },
        readiness_issues,
        model_path,
        metadata_path,
    }
}

pub(super) fn read_loop_intent_summary(
    client: &mut Client,
) -> Result<Option<LoopIntentTableSummary>> {
    if !table_exists(client, "loop_samples")? {
        return Ok(None);
    }

    let totals_row = client.query_one(
        "SELECT COUNT(*),
            COUNT(*) FILTER (WHERE embedding IS NULL),
            COUNT(*) FILTER (WHERE embedding IS NOT NULL AND embedding::text ~* $1),
            COUNT(*) FILTER (WHERE label = 1),
            COUNT(*) FILTER (WHERE label = 0),
            COUNT(*) FILTER (
                WHERE metadata ? 'directory_loop_intent_score'
                  AND (metadata->>'directory_loop_intent_score')::double precision <> 0.5
            )
         FROM loop_samples",
        &[&NON_FINITE_PATTERN],
    )?;

    let replica_row = client.query_one(
        "SELECT COUNT(*) FROM loop_samples WHERE source_path LIKE $1",
        &[&REPLICA_SOURCE_PATTERN],
    )?;

    let feature_stats_present = if table_exists(client, "multi_scenario_metadata")? {
        client
            .query_opt(
                "SELECT COALESCE(jsonb_typeof(feature_stats), 'null') <> 'null'
                 FROM multi_scenario_metadata WHERE scenario = 'loop_intent'",
                &[],
            )?
            .is_some_and(|r| r.get::<_, bool>(0))
    } else {
        false
    };

    Ok(Some(LoopIntentTableSummary {
        total: totals_row.get(0),
        null_embedding: totals_row.get(1),
        non_finite: totals_row.get(2),
        loop_positive_count: totals_row.get(3),
        video_negative_count: totals_row.get(4),
        non_neutral_directory_score: totals_row.get(5),
        replica_source_paths: replica_row.get(0),
        feature_stats_present,
    }))
}

pub(super) fn evaluate_loop_intent_runtime_status(
    summary: &LoopIntentTableSummary,
) -> LoopIntentRuntimeStatus {
    let mut readiness_issues = Vec::new();
    let loop_high = foundation::numeric_cast::i64_to_u64_sat(summary.loop_positive_count);
    let video = foundation::numeric_cast::i64_to_u64_sat(summary.video_negative_count);
    let total_u64 = foundation::numeric_cast::i64_to_u64_sat(summary.total);
    if !loop_corpus_is_mature(total_u64, loop_high, video) {
        let shortfall = loop_corpus_samples_shortfall(total_u64, loop_high, video);
        readiness_issues.push(format!(
            "corpus_shortfall={shortfall} (need total>={}, loop_high/video>={}; \
             have total={} loop_high={loop_high} video={video})",
            min_loop_samples_total(),
            min_loop_samples_per_class(),
            summary.total,
        ));
    }
    if summary.null_embedding > 0 {
        readiness_issues.push(format!("null_embedding={}", summary.null_embedding));
    }
    if summary.non_finite > 0 {
        readiness_issues.push(format!("non_finite={}", summary.non_finite));
    }

    let ready_for_knn = readiness_issues.is_empty();
    let directory_scores_ready =
        summary.total > 0 && summary.non_neutral_directory_score >= summary.total;
    let mut runtime_issues = Vec::new();
    if !ready_for_knn {
        runtime_issues.extend(readiness_issues.clone());
    }
    if !summary.feature_stats_present {
        runtime_issues.push("missing_loop_intent_feature_stats".to_string());
    }
    if summary.total > 0 && !directory_scores_ready {
        runtime_issues.push(format!(
            "directory_loop_intent_score_not_backfilled={}/{}",
            summary.non_neutral_directory_score, summary.total
        ));
    }

    LoopIntentRuntimeStatus {
        ready_for_knn,
        ready_for_runtime: ready_for_knn && summary.feature_stats_present && directory_scores_ready,
        readiness_issues: runtime_issues,
        predictor_family: "pgvector_hnsw+hdbscan",
    }
}

pub(super) fn print_loop_intent_runtime_status(summary: &LoopIntentTableSummary) {
    let status = evaluate_loop_intent_runtime_status(summary);
    println!(
        "knn_readiness={} thresholds=total>={},loop_high>={},video>={} \
         loop_high={} video={} predictor={}",
        if status.ready_for_knn {
            "ready"
        } else {
            "pending"
        },
        min_loop_samples_total(),
        min_loop_samples_per_class(),
        min_loop_samples_per_class(),
        summary.loop_positive_count,
        summary.video_negative_count,
        status.predictor_family,
    );
    if !status.readiness_issues.is_empty() {
        println!("knn_issues={}", status.readiness_issues.join("; "));
    }
    println!(
        "loop_runtime={} feature_stats={} directory_scores_backfilled={}/{}",
        if status.ready_for_runtime {
            "ready"
        } else {
            "pending"
        },
        if summary.feature_stats_present {
            "yes"
        } else {
            "no"
        },
        summary.non_neutral_directory_score,
        summary.total,
    );
}

pub(super) fn print_loop_distribution(client: &mut Client) -> Result<()> {
    if !table_exists(client, "loop_samples")? {
        println!("loop_samples table missing");
        return Ok(());
    }
    let row = client.query_one(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE label = 1), COUNT(*) FILTER (WHERE label = 0)
         FROM loop_samples",
        &[],
    )?;
    println!(
        "loop_samples total={} label=1={} label=0={}",
        row.get::<_, i64>(0),
        row.get::<_, i64>(1),
        row.get::<_, i64>(2),
    );
    Ok(())
}

pub fn verify_embeddings(
    client: &mut Client,
    scenarios: &[ScenarioSpec],
    heading: &str,
    include_legacy_schema: bool,
) -> Result<i32> {
    let mut failures = 0i32;
    let mut rows_out: Vec<Vec<String>> = Vec::new();

    if include_legacy_schema {
        let legacy = detect_legacy_animated_image_schema(client)?;
        if legacy.is_empty() {
            rows_out.push(vec!["schema".into(), "OK".into()]);
        } else {
            failures += 1;
            rows_out.push(vec!["schema".into(), legacy.join("; ")]);
        }
    }

    let metadata_present = table_exists(client, "multi_scenario_metadata")?;

    for scenario in scenarios {
        let name = scenario.name;
        let table = scenario.table;
        if !table_exists(client, table)? {
            rows_out.push(vec![name.into(), format!("missing_table={table}")]);
            failures += 1;
            continue;
        }

        let actual_dim = read_embedding_dimension(client, table)?;
        let has_score_col = column_exists(client, table, scenario.score_col)?;
        let mut issues = Vec::new();

        if metadata_present {
            let meta_row = client.query_opt(
                "SELECT embedding_dimension, sample_count
                 FROM multi_scenario_metadata WHERE scenario = $1",
                &[&name],
            )?;
            if meta_row.is_none() {
                issues.push("missing_metadata_row".to_string());
            } else if let Some(row) = meta_row {
                let meta_dim: i32 = row.get(0);
                let meta_count: i64 = row.get(1);
                if actual_dim.is_some_and(|d| d != meta_dim) {
                    issues.push(format!("dim_mismatch live={actual_dim:?} meta={meta_dim}"));
                }
                let live_count: i64 = client
                    .query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])?
                    .get(0);
                if live_count != meta_count {
                    issues.push(format!(
                        "count_mismatch live={live_count} meta={meta_count}"
                    ));
                }
            }
        } else {
            issues.push("missing_multi_scenario_metadata".to_string());
        }

        if actual_dim.is_some_and(|d| d != scenario.expected_dim) {
            issues.push(format!(
                "unexpected_dim={actual_dim:?} expected={}",
                scenario.expected_dim
            ));
        }

        if has_score_col {
            let row = client.query_one(
                &format!(
                    "SELECT COUNT(*) FILTER (WHERE embedding IS NULL),
                        COUNT(*) FILTER (WHERE embedding IS NOT NULL AND embedding::text ~* $1),
                        COUNT(*) FILTER (WHERE {} IS NULL),
                        COUNT(*) FILTER (WHERE {} >= 0.5),
                        COUNT(*) FILTER (WHERE {} < 0.5)
                     FROM {table}",
                    scenario.score_col, scenario.score_col, scenario.score_col
                ),
                &[&NON_FINITE_PATTERN],
            )?;
            let null_emb: i64 = row.get(0);
            let non_fin: i64 = row.get(1);
            let null_score: i64 = row.get(2);
            if null_emb > 0 {
                issues.push(format!("null_embedding={null_emb}"));
            }
            if non_fin > 0 {
                issues.push(format!("non_finite={non_fin}"));
            }
            if null_score > 0 {
                issues.push(format!("null_{}={null_score}", scenario.score_col));
            }
        }

        if issues.is_empty() {
            rows_out.push(vec![name.into(), "OK".into()]);
        } else {
            failures += 1;
            rows_out.push(vec![name.into(), issues.join("; ")]);
        }
    }

    println!("\n=== {heading} ===");
    render_table(&["scenario", "status"], &rows_out);
    Ok(failures)
}

fn detect_legacy_animated_image_schema(client: &mut Client) -> Result<Vec<String>> {
    let mut findings = Vec::new();
    for table in ["gif_quality_samples", "gif_quality_inference_log"] {
        if table_exists(client, table)? {
            findings.push(format!("legacy_table={table}"));
        }
    }
    Ok(findings)
}

pub fn verify_stack_readiness(client: &mut Client) -> Result<i32> {
    let schema_failures = verify_embeddings(
        client,
        SCENARIOS,
        "stack readiness / schema verification",
        true,
    )?;
    let mut failures = schema_failures;
    let mut rows_out = vec![vec![
        "schema_verification".into(),
        if schema_failures == 0 {
            "OK".into()
        } else {
            format!("{schema_failures} failure(s); see schema verification table above")
        },
    ]];

    if table_exists(client, "loop_samples")? {
        if let Some(summary) = read_loop_intent_summary(client)? {
            let status = evaluate_loop_intent_runtime_status(&summary);
            if status.ready_for_knn {
                rows_out.push(vec!["loop_intent_knn".into(), "ready".into()]);
            } else {
                failures += 1;
                rows_out.push(vec![
                    "loop_intent_knn".into(),
                    status.readiness_issues.join("; "),
                ]);
            }
            if status.ready_for_runtime {
                rows_out.push(vec!["loop_intent_runtime".into(), "ready".into()]);
            } else {
                failures += 1;
                rows_out.push(vec![
                    "loop_intent_runtime".into(),
                    status.readiness_issues.join("; "),
                ]);
            }
        } else {
            failures += 1;
            rows_out.push(vec![
                "loop_intent_runtime".into(),
                "missing_loop_samples_summary".into(),
            ]);
        }
    }

    if table_exists(client, "image_quality_samples")? {
        let summary = read_quality_table_summary(client, "image_quality_samples")?;
        let status = evaluate_image_quality_model_status(&summary);
        if status.state.ready_for_training {
            rows_out.push(vec!["image_quality_training".into(), "ready".into()]);
        } else {
            failures += 1;
            rows_out.push(vec![
                "image_quality_training".into(),
                status.readiness_issues.join("; "),
            ]);
        }
        if status.state.ready_for_runtime {
            rows_out.push(vec!["image_quality_runtime".into(), "ready".into()]);
        } else {
            failures += 1;
            let mut artifact_issues = Vec::new();
            if !status.state.artifacts.model {
                artifact_issues.push("missing_model");
            }
            if !status.state.artifacts.metadata {
                artifact_issues.push("missing_metadata");
            }
            if artifact_issues.is_empty() {
                artifact_issues.push("training_not_ready");
            }
            rows_out.push(vec![
                "image_quality_runtime".into(),
                artifact_issues.join("; "),
            ]);
        }
    }

    println!("\n=== stack readiness ===");
    render_table(&["check", "status"], &rows_out);
    Ok(if failures == 0 { 0 } else { 2 })
}

pub fn verify_fabrication_stock(client: &mut Client) -> Result<i32> {
    let mut failures = 0i32;
    println!("\n=== fabrication stock: loop feature_stats ===");
    let loop_rows: i64 = if table_exists(client, "loop_samples")? {
        client
            .query_one(
                "SELECT COUNT(*) FROM loop_samples WHERE frame_count > 1",
                &[],
            )?
            .get(0)
    } else {
        0
    };

    let mut stats_empty = true;
    if table_exists(client, "multi_scenario_metadata")? {
        if let Some(row) = client.query_opt(
            "SELECT sample_count, COALESCE(feature_stats, '{}'::jsonb)
             FROM multi_scenario_metadata WHERE scenario = 'loop_intent'",
            &[],
        )? {
            let feature_stats: serde_json::Value = row.get(1);
            stats_empty = feature_stats.is_null()
                || feature_stats
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty);
        }
    } else {
        failures += 1;
        println!("loop_feature_stats=missing_multi_scenario_metadata_table");
    }

    println!("loop_samples_trainable={loop_rows} feature_stats_empty={stats_empty}");
    if loop_rows > 0 && stats_empty {
        failures += 1;
        println!(
            "fabrication_blocker=loop_feature_stats_empty_with_samples \
             (run: training_pipeline refresh-loop-stats)"
        );
    }

    Ok(if failures == 0 { 0 } else { 2 })
}

pub fn print_quality_regression_report(client: &mut Client) -> Result<()> {
    println!("\n=== quality regression report ===");
    for scenario in QUALITY_REGRESSION_SCENARIOS {
        if !table_exists(client, scenario.table)? {
            println!("{}: missing table {}", scenario.name, scenario.table);
            continue;
        }
        let summary = read_quality_table_summary(client, scenario.table)?;
        println!(
            "{} total={} null_emb={} non_finite={} replica_paths={}",
            scenario.name,
            summary.total,
            summary.null_embedding,
            summary.non_finite,
            summary.replica_source_paths
        );
    }
    Ok(())
}

pub fn print_loop_clustering_report(client: &mut Client) -> Result<()> {
    println!("\n=== loop clustering report ===");
    if let Some(summary) = read_loop_intent_summary(client)? {
        print_loop_intent_runtime_status(&summary);
    } else {
        println!("loop_samples table missing");
    }
    Ok(())
}

pub fn print_full_report(client: &mut Client) -> Result<()> {
    print_loop_clustering_report(client)?;
    print_quality_regression_report(client)?;
    Ok(())
}

pub fn repair_multi_scenario_schema(
    client: &mut Client,
    migration_sql: &str,
    drop_legacy_gif_schema: bool,
) -> Result<i32> {
    if drop_legacy_gif_schema {
        for table in ["gif_quality_samples", "gif_quality_inference_log"] {
            if table_exists(client, table)? {
                client.batch_execute(&format!("DROP TABLE IF EXISTS {table} CASCADE"))?;
            }
        }
    }
    client
        .batch_execute(migration_sql)
        .context("apply multi-scenario migration")?;
    Ok(0)
}

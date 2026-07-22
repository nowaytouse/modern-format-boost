//! Runtime asset finalization orchestration (mirrors `training_pipeline.py`
//! finalize paths).

use crate::infra::corpus_thresholds::{
    min_loop_samples_per_class, min_loop_samples_total, min_quality_samples_per_class,
    min_quality_samples_total,
};
use crate::training_pipeline::audit::{
    self, evaluate_image_quality_model_status, evaluate_loop_intent_runtime_status,
    print_loop_intent_runtime_status, read_loop_intent_summary, read_quality_table_summary,
};
use crate::training_pipeline::delegate::{
    project_root, run_dev_bin, run_foundation_bin, run_python_script, run_run_training_batch,
};
use anyhow::{Context, Result};
use postgres::{Client, NoTls};

const QUALITY_MODEL_SCRIPT: &str = "crates/dev/scripts/quality_regression_model.py";
const LOOP_CLUSTERING_SCRIPT: &str = "crates/dev/scripts/loop_intent_clustering.py";

pub fn refresh_loop_stats(connstr: &str) -> Result<i32> {
    let root = project_root()?;
    run_foundation_bin(&root, "refresh_stats", &[], connstr)
}

pub fn repair_loop_probe_metadata(connstr: &str) -> Result<i32> {
    eprintln!(
        "[WARN] repair-loop-probe-metadata re-reads media files on disk; this is NOT run_training \
         / full re-ingest."
    );
    let root = project_root()?;
    run_foundation_bin(&root, "repair_loop_probe", &[], connstr)
}

pub fn run_training_batch(connstr: &str) -> Result<i32> {
    eprintln!("Legacy `train` → run_training (Rust) --use-api --fill-runtime-assets");
    let root = project_root()?;
    run_run_training_batch(&root, connstr)
}

pub fn train_image_quality_model(connstr: &str) -> Result<i32> {
    let root = project_root()?;
    run_python_script(
        &root,
        QUALITY_MODEL_SCRIPT,
        &["train-image-quality", "--connstr", connstr],
        Some(connstr),
    )
}

pub fn show_image_quality_model_paths() -> Result<i32> {
    use crate::infra::corpus_thresholds::{
        min_quality_samples_per_class, min_quality_samples_total,
    };
    use crate::training_pipeline::audit::{
        default_image_quality_metadata_path, default_image_quality_model_path,
    };
    let payload = serde_json::json!({
        "model": default_image_quality_model_path(),
        "metadata": default_image_quality_metadata_path(),
        "scenario": "image_quality",
        "feature_schema": "image_quality_lgbm_v1",
        "min_samples_total": min_quality_samples_total(),
        "min_samples_per_class": min_quality_samples_per_class(),
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(0)
}

fn backfill_loop_directory_scores(connstr: &str, skip_refresh: bool) -> Result<i32> {
    let root = project_root()?;
    let mut args = vec!["--connstr", connstr];
    if skip_refresh {
        args.push("--no-refresh-stats");
    }
    run_dev_bin(&root, "backfill_directory_scores", &args)
}

fn run_loop_hdbscan_clustering(connstr: &str) -> Result<i32> {
    eprintln!("  [CLUSTER] Running loop_intent HDBSCAN clustering...");
    let root = project_root()?;
    run_python_script(
        &root,
        LOOP_CLUSTERING_SCRIPT,
        &["--connstr", connstr],
        Some(connstr),
    )
}

pub fn finalize_loop_intent_assets(connstr: &str) -> Result<i32> {
    let mut client =
        Client::connect(connstr, NoTls).context("connect postgres for loop finalize")?;

    println!("\n=== loop intent finalize ===");
    let summary = read_loop_intent_summary(&mut client)?;
    let Some(summary) = summary else {
        println!("finalize_blocked=missing_loop_samples_table");
        return Ok(2);
    };

    audit::print_loop_distribution(&mut client)?;
    print_loop_intent_runtime_status(&summary);
    let status = evaluate_loop_intent_runtime_status(&summary);

    if summary.total == 0 {
        println!("finalize_blocked=no_loop_samples_rows");
        return Ok(2);
    }

    drop(client);

    let refresh_exit = refresh_loop_stats(connstr)?;
    if refresh_exit != 0 {
        println!("finalize_failed=refresh_loop_stats exit={refresh_exit}");
        return Ok(refresh_exit);
    }

    let backfill_exit = backfill_loop_directory_scores(connstr, true)?;
    if backfill_exit != 0 {
        println!("finalize_failed=backfill_directory_scores exit={backfill_exit}");
        return Ok(backfill_exit);
    }

    let cluster_exit = run_loop_hdbscan_clustering(connstr)?;
    if cluster_exit != 0 {
        println!("finalize_failed=loop_hdbscan_clustering exit={cluster_exit}");
        return Ok(cluster_exit);
    }

    let mut client =
        Client::connect(connstr, NoTls).context("reconnect postgres after loop finalize")?;
    let refreshed = read_loop_intent_summary(&mut client)?;
    let final_status = if let Some(ref refreshed) = refreshed {
        print_loop_intent_runtime_status(refreshed);
        evaluate_loop_intent_runtime_status(refreshed)
    } else {
        status
    };

    if final_status.ready_for_runtime {
        println!("finalize_result=loop_intent_runtime_ready");
        return Ok(0);
    }
    if final_status.ready_for_knn {
        println!("finalize_result=loop_stats_refreshed_knn_pending_runtime_hygiene");
        return Ok(2);
    }
    println!("finalize_result=loop_stats_refreshed_corpus_immature");
    Ok(2)
}

pub fn finalize_image_quality_model(connstr: &str, install_deps: bool) -> Result<i32> {
    let root = crate::training_pipeline::delegate::project_root()?;
    crate::training_pipeline::delegate::ensure_python_training_requirements(&root, install_deps)?;
    let mut client =
        Client::connect(connstr, NoTls).context("connect postgres for image_quality finalize")?;

    println!("\n=== image quality finalize ===");
    let summary = read_quality_table_summary(&mut client, "image_quality_samples")?;
    audit::print_image_quality_model_status(&summary);
    let status = evaluate_image_quality_model_status(&summary);

    if !status.state.ready_for_training {
        println!("finalize_blocked={}", status.readiness_issues.join("; "));
        return Ok(2);
    }

    drop(client);

    let train_exit = train_image_quality_model(connstr)?;
    if train_exit != 0 {
        return Ok(train_exit);
    }

    let model_path = audit::default_image_quality_model_path();
    let metadata_path = audit::default_image_quality_metadata_path();
    if !model_path.is_file() || !metadata_path.is_file() {
        println!(
            "finalize_failed=missing_artifacts model_exists={} metadata_exists={}",
            model_path.is_file(),
            metadata_path.is_file()
        );
        return Ok(1);
    }

    let artifact_state = if !status.state.artifacts.model && !status.state.artifacts.metadata {
        "created_model_and_metadata"
    } else if !status.state.artifacts.model {
        "created_model"
    } else if !status.state.artifacts.metadata {
        "created_metadata"
    } else {
        "ready"
    };
    println!("finalize_result={artifact_state}");
    println!(
        "runtime_ready=model={} metadata={}",
        model_path.display(),
        metadata_path.display()
    );
    Ok(0)
}

fn combine_exit_codes(codes: &[i32]) -> i32 {
    if codes.contains(&1) {
        1
    } else if codes.contains(&2) {
        2
    } else {
        0
    }
}

pub fn finalize_runtime_assets(connstr: &str, install_missing_python_deps: bool) -> Result<i32> {
    let mut multi_exit = 2i32;
    for pass_idx in 1..=5usize {
        eprintln!("  [INFO] Runtime finalize pass {pass_idx}/5...");
        let loop_exit = finalize_loop_intent_assets(connstr)?;
        let quality_exit = finalize_image_quality_model(connstr, install_missing_python_deps)?;
        multi_exit = combine_exit_codes(&[loop_exit, quality_exit]);
        if multi_exit == 1 {
            return Ok(multi_exit);
        }
        if multi_exit == 0 {
            if pass_idx > 1 {
                eprintln!("  [INFO] Runtime assets converged after {pass_idx} finalize pass(es).");
            }
            break;
        }
    }

    if multi_exit == 2 {
        eprintln!(
            "  [INFO] After finalize passes, one or more runtime families still pending maturity."
        );
    }
    Ok(multi_exit)
}

pub fn print_ingest_guidance(dataset_path: &str) {
    println!("Batch ingestion entrypoint:");
    println!("  cargo run --locked -p dev --bin run_training -- --use-api --fill-runtime-assets");
    println!("Dataset hint: {dataset_path}");
    println!(
        "Thresholds: loop total>={} per_class>={}; quality total>={} per_class>={}",
        min_loop_samples_total(),
        min_loop_samples_per_class(),
        min_quality_samples_total(),
        min_quality_samples_per_class(),
    );
}

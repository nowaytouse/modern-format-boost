//! Rust port of `crates/dev/scripts/training_pipeline.py`.
//!
//! Python script is retained as compatibility reference; this binary is the
//! preferred entry for Rust callers and CI.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use dev::training_pipeline::{
    LOOP_CLUSTERING_SCENARIOS, QUALITY_REGRESSION_SCENARIOS, SCENARIOS,
    finalize_image_quality_model, finalize_loop_intent_assets, finalize_runtime_assets,
    print_full_report, print_loop_clustering_report, print_quality_regression_report, project_root,
    refresh_loop_stats, repair_loop_probe_metadata, repair_multi_scenario_schema, resolve_connstr,
    run_training_batch, show_image_quality_model_paths, train_image_quality_model,
    verify_embeddings, verify_fabrication_stock, verify_stack_readiness,
};
use postgres::{Client, NoTls};
use std::fs;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "training_pipeline",
    about = "Training database audit and runtime asset orchestration (Rust)"
)]
struct Cli {
    #[arg(long)]
    connstr: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Compatibility alias: batch ingestion via run_training
    Train,
    /// Compatibility alias: combined audit + verify-embeddings
    Evaluate,
    /// Compatibility alias: refresh-loop-stats
    ExportStats,
    /// Combined task-family report
    Report,
    ReportQualityRegression,
    ReportLoopClustering,
    VerifyEmbeddings,
    VerifyQualityRegression,
    VerifyStackReadiness,
    VerifyFabricationStock,
    VerifyLoopClustering,
    RefreshLoopStats,
    RepairLoopProbeMetadata,
    TrainImageQualityModel,
    FinalizeImageQualityModel {
        #[arg(long)]
        install_missing_python_deps: bool,
    },
    FinalizeLoopIntent,
    FinalizeRuntimeAssets {
        #[arg(long)]
        install_missing_python_deps: bool,
    },
    RepairMultiScenarioSchema {
        #[arg(long)]
        drop_legacy_gif_schema: bool,
    },
    ShowImageQualityModelPaths,
    Ingest {
        path: String,
    },
}

fn connect(connstr: &str) -> Result<Client> {
    Client::connect(connstr, NoTls).with_context(|| format!("PostgreSQL connect ({connstr})"))
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let connstr = resolve_connstr(cli.connstr.as_deref());

    let code = match cli.command {
        Command::Train => run_training_batch(&connstr)?,
        Command::ExportStats | Command::RefreshLoopStats => refresh_loop_stats(&connstr)?,
        Command::RepairLoopProbeMetadata => repair_loop_probe_metadata(&connstr)?,
        Command::TrainImageQualityModel => train_image_quality_model(&connstr)?,
        Command::FinalizeImageQualityModel {
            install_missing_python_deps,
        } => finalize_image_quality_model(&connstr, install_missing_python_deps)?,
        Command::FinalizeLoopIntent => finalize_loop_intent_assets(&connstr)?,
        Command::FinalizeRuntimeAssets {
            install_missing_python_deps,
        } => finalize_runtime_assets(&connstr, install_missing_python_deps)?,
        Command::ShowImageQualityModelPaths => show_image_quality_model_paths()?,
        Command::Ingest { path } => {
            dev::training_pipeline::orchestrate::print_ingest_guidance(&path);
            0
        }
        Command::RepairMultiScenarioSchema {
            drop_legacy_gif_schema,
        } => {
            let root = project_root()?;
            let sql_path = root.join("migrations/001_multi_scenario_embedding.sql");
            let sql = fs::read_to_string(&sql_path)
                .with_context(|| format!("read migration {}", sql_path.display()))?;
            let mut client = connect(&connstr)?;
            repair_multi_scenario_schema(&mut client, &sql, drop_legacy_gif_schema)?
        }
        Command::Report => {
            let mut client = connect(&connstr)?;
            print_full_report(&mut client)?;
            0
        }
        Command::ReportQualityRegression => {
            let mut client = connect(&connstr)?;
            print_quality_regression_report(&mut client)?;
            0
        }
        Command::ReportLoopClustering => {
            let mut client = connect(&connstr)?;
            print_loop_clustering_report(&mut client)?;
            0
        }
        Command::Evaluate => {
            let mut client = connect(&connstr)?;
            println!("Legacy `evaluate` → combined report + verify-embeddings");
            print_full_report(&mut client)?;
            verify_embeddings(&mut client, SCENARIOS, "embedding verification", true)?
        }
        Command::VerifyEmbeddings => {
            let mut client = connect(&connstr)?;
            verify_embeddings(&mut client, SCENARIOS, "embedding verification", true)?
        }
        Command::VerifyQualityRegression => {
            let mut client = connect(&connstr)?;
            verify_embeddings(
                &mut client,
                QUALITY_REGRESSION_SCENARIOS,
                "quality regression verification",
                true,
            )?
        }
        Command::VerifyLoopClustering => {
            let mut client = connect(&connstr)?;
            verify_embeddings(
                &mut client,
                LOOP_CLUSTERING_SCENARIOS,
                "loop clustering verification",
                false,
            )?
        }
        Command::VerifyStackReadiness => {
            let mut client = connect(&connstr)?;
            verify_stack_readiness(&mut client)?
        }
        Command::VerifyFabricationStock => {
            let mut client = connect(&connstr)?;
            verify_fabrication_stock(&mut client)?
        }
    };

    Ok(ExitCode::from(code.clamp(0, 255) as u8))
}

use crate::run_training::types::{FillAssetsConfig, TrainingMode};
use crate::training_pipeline::{
    finalize_image_quality_model, finalize_loop_intent_assets, print_loop_clustering_report,
    print_quality_regression_report, verify_stack_readiness,
};
use anyhow::{Context, Result};
use postgres::{Client, NoTls};

pub fn fill_runtime_assets(conn_str: &str, config: FillAssetsConfig) -> Result<i32> {
    let include_image_quality = matches!(
        config.training_mode,
        TrainingMode::All | TrainingMode::Static
    );
    let include_loop_intent =
        matches!(config.training_mode, TrainingMode::All | TrainingMode::Loop);

    if include_image_quality && !config.state.saw_image_quality {
        eprintln!(
            "  [INFO] No new image_quality samples routed; still checking/filling LightGBM when \
             mature."
        );
    }
    if include_loop_intent && !config.state.saw_loop_samples {
        eprintln!("  [INFO] No new loop_intent samples routed; still refreshing loop KNN stats.");
    }
    match config.training_mode {
        TrainingMode::Static => {
            eprintln!("  [INFO] training_mode=static: runtime fill skips loop_intent");
        }
        TrainingMode::Loop => {
            eprintln!("  [INFO] training_mode=loop: runtime fill skips image_quality LightGBM");
        }
        TrainingMode::All => {}
    }

    let mut multi_exit = 2i32;
    if include_image_quality || include_loop_intent {
        for pass_idx in 1..=5usize {
            eprintln!("  [INFO] Runtime finalize pass {pass_idx}/5...");
            let mut pass_exits = Vec::new();
            if include_loop_intent {
                pass_exits.push(finalize_loop_intent_assets(conn_str)?);
            }
            if include_image_quality {
                pass_exits.push(finalize_image_quality_model(
                    conn_str,
                    config.actions.install_deps,
                )?);
            }
            multi_exit = if pass_exits.contains(&1) {
                1
            } else if pass_exits.contains(&2) {
                2
            } else {
                0
            };
            if multi_exit == 1 {
                return Ok(multi_exit);
            }
            if multi_exit == 0 {
                if pass_idx > 1 {
                    eprintln!(
                        "  [INFO] Runtime assets converged after {pass_idx} finalize pass(es)."
                    );
                }
                break;
            }
        }
        if multi_exit == 2 {
            eprintln!(
                "  [INFO] After finalize passes, one or more runtime families still pending \
                 maturity."
            );
        }
    }

    if include_loop_intent || include_image_quality {
        let mut client =
            Client::connect(conn_str, NoTls).context("connect for runtime asset reports")?;
        if include_loop_intent {
            print_loop_clustering_report(&mut client)?;
        }
        if include_image_quality {
            print_quality_regression_report(&mut client)?;
        }
    }

    if config.actions.verify_after {
        let mut client =
            Client::connect(conn_str, NoTls).context("connect for stack readiness verify")?;
        return verify_stack_readiness(&mut client);
    }
    if multi_exit == 2 {
        eprintln!("  [PENDING] One or more runtime families not fully mature yet.");
        return Ok(2);
    }
    Ok(0)
}

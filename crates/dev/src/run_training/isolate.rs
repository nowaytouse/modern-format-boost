use crate::run_training::scanner::passes_file_quality_filter;
use crate::run_training::types::{Args, FillAssetsConfig, Sample, TrainingMode};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

const IMAGE_EXTS: &[&str] = &[
    "jpeg", "png", "webp", "gif", "avif", "heic", "jxl", "tiff", "bmp", "jpg", "tif",
];
const ANIMATED_IMAGE_EXTS: &[&str] = &["gif", "webp", "apng", "avif"];
const VIDEO_EXTS: &[&str] = &["mp4", "mov", "webm", "mkv", "avi"];

fn guess_extension_from_content_type(content_type: &str) -> Option<&'static str> {
    let content_type = content_type.to_lowercase();
    let known: &[(&str, &str)] = &[
        ("image/gif", "gif"),
        ("image/jpeg", "jpg"),
        ("image/png", "png"),
        ("image/webp", "webp"),
        ("image/avif", "avif"),
        ("image/heic", "heic"),
        ("image/heif", "heif"),
        ("image/tiff", "tiff"),
        ("image/bmp", "bmp"),
        ("video/mp4", "mp4"),
        ("video/quicktime", "mov"),
        ("video/webm", "webm"),
        ("video/x-matroska", "mkv"),
        ("video/x-msvideo", "avi"),
    ];
    for (ct, ext) in known {
        if content_type == *ct {
            return Some(*ext);
        }
    }
    None
}

fn detect_media_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "jpg" => "jpeg".to_string(),
        "tif" => "tiff".to_string(),
        _ => ext,
    }
}

fn is_label_conflict(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("label_conflict:")
        || m.contains("immutable once set")
        || m.contains("quality_label is immutable")
        || m.contains("loop_samples.label is immutable")
        || m.contains("quality_score is immutable once set")
}

fn should_route_to_animated_image_quality(ext: &str) -> bool {
    ANIMATED_IMAGE_EXTS.contains(&ext)
}

fn get_scenarios(path: &Path, no_loop: bool) -> Vec<String> {
    let ext = detect_media_extension(path);
    let mut scenarios = Vec::new();

    let routes_to_animated = ANIMATED_IMAGE_EXTS.contains(&ext.as_str())
        && !no_loop
        && should_route_to_animated_image_quality(&ext);

    if IMAGE_EXTS.contains(&ext.as_str()) && !routes_to_animated {
        scenarios.push("image_quality".to_string());
    }

    if !no_loop {
        if routes_to_animated {
            scenarios.push("animated_image_quality".to_string());
        }
        if VIDEO_EXTS.contains(&ext.as_str()) {
            scenarios.push("video_quality".to_string());
        }
    }

    scenarios
}

fn filter_scenarios(scenarios: Vec<String>, mode: TrainingMode) -> Vec<String> {
    if mode == TrainingMode::Static {
        scenarios
            .into_iter()
            .filter(|s| s == "image_quality")
            .collect()
    } else {
        scenarios
    }
}

use crate::training_pipeline::{
    finalize_image_quality_model, finalize_loop_intent_assets, print_loop_clustering_report,
    print_quality_regression_report, verify_stack_readiness,
};

fn combine_finalize_exit_codes(codes: &[i32]) -> i32 {
    if codes.contains(&1) {
        1
    } else if codes.contains(&2) {
        2
    } else {
        0
    }
}

fn fill_runtime_assets(conn_str: &str, config: FillAssetsConfig) -> Result<i32> {
    let include_img =
        config.training_mode == TrainingMode::All || config.training_mode == TrainingMode::Static;
    let include_loop =
        config.training_mode == TrainingMode::All || config.training_mode == TrainingMode::Loop;

    if include_img && !config.state.saw_image_quality {
        eprintln!("  [INFO] No new image_quality samples routed; checking existing model.");
    }
    if include_loop && !config.state.saw_loop_samples {
        eprintln!("  [INFO] No new loop_intent samples routed; refreshing existing stats.");
    }

    let mut multi_exit = 2;
    if include_img || include_loop {
        for pass_idx in 1..=5 {
            eprintln!("  [INFO] Runtime finalize pass {pass_idx}/5...");
            let mut pass_exits = Vec::new();
            if include_loop {
                pass_exits.push(finalize_loop_intent_assets(conn_str)?);
            }
            if include_img {
                pass_exits.push(finalize_image_quality_model(
                    conn_str,
                    config.actions.install_deps,
                )?);
            }

            multi_exit = combine_finalize_exit_codes(&pass_exits);
            if multi_exit == 1 {
                return Ok(multi_exit);
            }
            if multi_exit == 0 {
                if pass_idx > 1 {
                    eprintln!("  [INFO] Runtime assets converged after {pass_idx} pass(es).");
                }
                break;
            }
        }
    }

    if include_loop {
        let mut client = postgres::Client::connect(conn_str, postgres::NoTls)
            .context("connect for loop clustering report")?;
        print_loop_clustering_report(&mut client)?;
    }
    if include_img {
        let mut client = postgres::Client::connect(conn_str, postgres::NoTls)
            .context("connect for quality regression report")?;
        print_quality_regression_report(&mut client)?;
    }

    if config.actions.verify_after {
        let mut client = postgres::Client::connect(conn_str, postgres::NoTls)
            .context("connect for stack readiness")?;
        return verify_stack_readiness(&mut client);
    }
    Ok(multi_exit)
}

fn ingest_quality_group(
    paths: &[PathBuf],
    label: &str,
    scenario: &str,
    verbose: bool,
    use_api: bool,
    conn_str: &str,
) -> Result<(usize, usize, usize)> {
    if paths.is_empty() {
        return Ok((0, 0, 0));
    }

    if use_api {
        let mut success_count = 0;
        let mut fail_other = 0;
        let mut fail_lc = 0;

        let paths_json = serde_json::to_string(&paths).unwrap_or_default();
        let conn_c = CString::new(conn_str.to_string()).unwrap();
        let paths_c = CString::new(paths_json).unwrap();
        let label_c = CString::new(label.to_string()).unwrap();
        let scenario_c = CString::new(scenario.to_string()).unwrap();

        let batch_result = unsafe {
            foundation::c_api::ingest_media_samples_batch(
                conn_c.as_ptr(),
                paths_c.as_ptr(),
                label_c.as_ptr(),
                scenario_c.as_ptr(),
            )
        };

        if batch_result < 0 {
            eprintln!("     [FAIL] C-API batch failed with code {batch_result}");
            return Ok((0, paths.len(), 0));
        } else if foundation::numeric_cast::i32_to_usize_sat(batch_result) == paths.len() {
            if verbose {
                for p in paths {
                    eprintln!("     [OK] {}/{} {}", scenario, label, p.display());
                }
            } else {
                eprintln!("     [OK] C-API batch {scenario}/{label} n={batch_result}");
            }
            return Ok((
                foundation::numeric_cast::i32_to_usize_sat(batch_result),
                0,
                0,
            ));
        }

        for path in paths {
            let path_json = serde_json::to_string(&vec![path]).unwrap_or_default();
            let path_c = CString::new(path_json).unwrap();
            let res = unsafe {
                foundation::c_api::ingest_media_samples_batch(
                    conn_c.as_ptr(),
                    path_c.as_ptr(),
                    label_c.as_ptr(),
                    scenario_c.as_ptr(),
                )
            };
            if res == 1 {
                success_count += 1;
                if verbose {
                    eprintln!("     [OK] {}/{} {}", scenario, label, path.display());
                }
            } else {
                fail_other += 1;
                let last_err_ptr = unsafe { foundation::c_api::mfb_last_ingest_error() };
                let err_msg = if last_err_ptr.is_null() {
                    format!("Code {res}")
                } else {
                    unsafe { std::ffi::CStr::from_ptr(last_err_ptr) }
                        .to_string_lossy()
                        .to_string()
                };
                if err_msg.contains("conflict") {
                    fail_lc += 1;
                    eprintln!(
                        "     [FAIL:label_conflict] {}/{} {}: {}",
                        scenario,
                        label,
                        path.display(),
                        err_msg
                    );
                } else {
                    eprintln!(
                        "     [FAIL] {}/{} {}: {}",
                        scenario,
                        label,
                        path.display(),
                        err_msg
                    );
                }
            }
        }
        return Ok((success_count, fail_other, fail_lc));
    }

    let mut success_count = 0;
    let mut fail_other = 0;
    let mut fail_lc = 0;

    let train_bin = match std::env::current_dir() {
        Ok(d) => {
            let candidate = d.join("target/release/train_quality");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        }
        Err(err) => {
            eprintln!("[ISOLATE] current_dir failed for train_quality bin: {err}");
            None
        }
    };

    for path in paths {
        let mut cmd = if let Some(ref bin) = train_bin {
            let mut c = Command::new(bin);
            c.arg(path)
                .arg("--label")
                .arg(label)
                .arg("--scenario")
                .arg(scenario)
                .arg("--conn")
                .arg(conn_str);
            c
        } else {
            let mut c = Command::new("cargo");
            c.args([
                "run",
                "--release",
                "-p",
                "foundation",
                "--bin",
                "train_quality",
                "--",
            ])
            .arg(path)
            .arg("--label")
            .arg(label)
            .arg("--scenario")
            .arg(scenario)
            .arg("--conn")
            .arg(conn_str);
            c
        };
        let output = cmd.output()?;
        if output.status.success() {
            if verbose {
                eprintln!("     [OK] {}/{} {}", scenario, label, path.display());
            }
            success_count += 1;
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = stderr.trim();
            if is_label_conflict(msg) {
                eprintln!(
                    "     [FAIL:label_conflict] {}/{} {}: {}",
                    scenario,
                    label,
                    path.display(),
                    msg
                );
                fail_lc += 1;
            } else {
                eprintln!(
                    "     [FAIL] {}/{} {}: {}",
                    scenario,
                    label,
                    path.display(),
                    msg
                );
                fail_other += 1;
            }
        }
    }
    Ok((success_count, fail_other, fail_lc))
}

fn ingest_loop_group(
    paths: &[PathBuf],
    loop_intent_label: &str,
    verbose: bool,
    use_api: bool,
    conn_str: &str,
) -> Result<(usize, usize, usize)> {
    if paths.is_empty() {
        return Ok((0, 0, 0));
    }

    // Convert to C-API if requested
    if use_api {
        let mut success_count = 0;
        let mut fail_other = 0;
        let mut fail_lc = 0;

        let paths_json = serde_json::to_string(&paths).unwrap_or_default();
        let conn_c = CString::new(conn_str.to_string()).unwrap();
        let paths_c = CString::new(paths_json).unwrap();
        let label_c = CString::new(loop_intent_label.to_string()).unwrap();
        let scenario_c = CString::new("loop_intent").unwrap();

        let batch_result = unsafe {
            foundation::c_api::ingest_media_samples_batch(
                conn_c.as_ptr(),
                paths_c.as_ptr(),
                label_c.as_ptr(),
                scenario_c.as_ptr(),
            )
        };

        if batch_result < 0 {
            eprintln!("     [FAIL] C-API batch failed with code {batch_result}");
            return Ok((0, paths.len(), 0));
        } else if foundation::numeric_cast::i32_to_usize_sat(batch_result) == paths.len() {
            if verbose {
                for p in paths {
                    eprintln!(
                        "     [OK] loop_intent/{} {}",
                        loop_intent_label,
                        p.display()
                    );
                }
            } else {
                eprintln!("     [OK] C-API batch loop_intent/{loop_intent_label} n={batch_result}");
            }
            return Ok((
                foundation::numeric_cast::i32_to_usize_sat(batch_result),
                0,
                0,
            ));
        }

        for path in paths {
            let path_json = serde_json::to_string(&vec![path]).unwrap_or_default();
            let path_c = CString::new(path_json).unwrap();
            let res = unsafe {
                foundation::c_api::ingest_media_samples_batch(
                    conn_c.as_ptr(),
                    path_c.as_ptr(),
                    label_c.as_ptr(),
                    scenario_c.as_ptr(),
                )
            };
            if res == 1 {
                success_count += 1;
                if verbose {
                    eprintln!(
                        "     [OK] loop_intent/{} {}",
                        loop_intent_label,
                        path.display()
                    );
                }
            } else {
                fail_other += 1;
                let last_err_ptr = unsafe { foundation::c_api::mfb_last_ingest_error() };
                let err_msg = if last_err_ptr.is_null() {
                    format!("Code {res}")
                } else {
                    unsafe { std::ffi::CStr::from_ptr(last_err_ptr) }
                        .to_string_lossy()
                        .to_string()
                };
                if err_msg.contains("conflict") {
                    fail_lc += 1;
                    eprintln!(
                        "     [FAIL:label_conflict] loop_intent/{} {}: {}",
                        loop_intent_label,
                        path.display(),
                        err_msg
                    );
                } else {
                    eprintln!(
                        "     [FAIL] loop_intent/{} {}: {}",
                        loop_intent_label,
                        path.display(),
                        err_msg
                    );
                }
            }
        }
        return Ok((success_count, fail_other, fail_lc));
    }

    let mut success_count = 0;
    let mut fail_other = 0;
    let mut fail_lc = 0;

    let train_bin = match std::env::current_dir() {
        Ok(d) => {
            let candidate = d.join("target/release/train_knn");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        }
        Err(err) => {
            eprintln!("[ISOLATE] current_dir failed for train_knn bin: {err}");
            None
        }
    };

    // Per Python: batch the entire loop_paths dir first, then retry per-file
    let loop_root = paths[0]
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| paths[0].parent().unwrap_or(paths[0].as_path()));

    let mut batch_cmd = if let Some(ref bin) = train_bin {
        let mut c = Command::new(bin);
        c.arg(loop_root).arg("--conn").arg(conn_str);
        if loop_intent_label != "auto" {
            c.arg("--label").arg(loop_intent_label);
        }
        c
    } else {
        let mut c = Command::new("cargo");
        c.args([
            "run",
            "--release",
            "-p",
            "foundation",
            "--bin",
            "train_knn",
            "--",
        ])
        .arg(loop_root)
        .arg("--conn")
        .arg(conn_str);
        if loop_intent_label != "auto" {
            c.arg("--label").arg(loop_intent_label);
        }
        c
    };
    let batch_output = batch_cmd.output()?;
    if batch_output.status.success() {
        if verbose {
            for path in paths {
                eprintln!("     [OK] loop/{} {}", loop_intent_label, path.display());
            }
        } else {
            eprintln!("     [OK] train_knn batch loop_intent n={}", paths.len());
        }
        return Ok((paths.len(), 0, 0));
    }

    // batch failed — retry per-file
    let batch_stderr = String::from_utf8_lossy(&batch_output.stderr);
    eprintln!(
        "     [WARN] train_knn batch failed for {}: {}; retrying per sample",
        loop_root.display(),
        batch_stderr.trim()
    );

    for path in paths {
        let parent = path.parent().unwrap_or(path.as_path());
        let mut cmd = if let Some(ref bin) = train_bin {
            let mut c = Command::new(bin);
            c.arg(parent).arg("--conn").arg(conn_str);
            if loop_intent_label != "auto" {
                c.arg("--label").arg(loop_intent_label);
            }
            c
        } else {
            let mut c = Command::new("cargo");
            c.args([
                "run",
                "--release",
                "-p",
                "foundation",
                "--bin",
                "train_knn",
                "--",
            ])
            .arg(parent)
            .arg("--conn")
            .arg(conn_str);
            if loop_intent_label != "auto" {
                c.arg("--label").arg(loop_intent_label);
            }
            c
        };
        let output = cmd.output()?;
        if output.status.success() {
            eprintln!("     [OK] loop/{} {}", loop_intent_label, path.display());
            success_count += 1;
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = stderr.trim();
            if is_label_conflict(msg) {
                eprintln!(
                    "     [FAIL:label_conflict] loop/{} {}: {}",
                    loop_intent_label,
                    path.display(),
                    msg
                );
                fail_lc += 1;
            } else {
                eprintln!(
                    "     [FAIL] loop/{} {}: {}",
                    loop_intent_label,
                    path.display(),
                    msg
                );
                fail_other += 1;
            }
        }
    }
    Ok((success_count, fail_other, fail_lc))
}

pub fn run_training_isolated(all_samples: &[Sample], args: &Args) -> Result<(usize, usize, usize)> {
    eprintln!(
        "\n🚀 ZERO-POLLUTION ENGINE: Version 6.4 (Samples: {})",
        all_samples.len()
    );

    let conn_str = std::env::var("MFB_PG_CONNSTR")
        .unwrap_or_else(|_| "postgres://localhost/mfb_training".to_string());
    if conn_str.trim().is_empty() {
        bail!("invalid or empty PostgreSQL connection string in MFB_PG_CONNSTR");
    }

    let mut total_success = 0;
    let mut total_fail_other = 0;
    let mut total_fail_label_conflict = 0;
    let mut _total_skip = 0;
    let mut saw_image_quality = false;
    let mut saw_loop_samples = false;

    let batch_size = 400;
    for (i, batch) in all_samples.chunks(batch_size).enumerate() {
        eprintln!(
            "  📦 Batch {}/{}...",
            i + 1,
            all_samples.len().div_ceil(batch_size)
        );

        let tmp_dir = tempdir()?;
        let tmp_path = tmp_dir.path();

        let mut quality_tasks: HashMap<(String, String), Vec<PathBuf>> = HashMap::new();
        let mut loop_paths: Vec<PathBuf> = Vec::new();

        let mut replica_ok = 0;
        let mut replica_fail = 0;

        eprintln!(
            "     [PHASE] 1/2 replica+routing batch_size={}",
            batch.len()
        );
        for (j, s) in batch.iter().enumerate() {
            let display_name = Path::new(&s.path_or_url).file_name().unwrap_or_default();
            let dest_dir = tmp_path.join(&s.base_label).join(format!("sample_{j:05}"));
            fs::create_dir_all(&dest_dir)?;
            let dest = dest_dir.join(display_name);
            let mut final_dest = dest.clone();

            if s.is_remote {
                eprintln!(
                    "     [REMOTE] downloading {} to {}",
                    s.path_or_url,
                    dest.display()
                );
                let staging = dest.with_extension("mfb_part");
                let headers_file = staging.with_extension("headers");
                let mut curl_cmd = Command::new("curl");
                curl_cmd
                    .args([
                        "-L",
                        "-A",
                        "ModernFormatBoost-Training/1.0",
                        "-sS",
                        "--connect-timeout",
                        "10",
                        "-D",
                    ])
                    .arg(&headers_file)
                    .arg("-o")
                    .arg(&staging)
                    .arg(s.path_or_url.clone());
                match curl_cmd.status() {
                    Ok(status) if status.success() => {
                        if dest.extension().is_none() && headers_file.exists() {
                            let headers = fs::read_to_string(&headers_file).unwrap_or_default();
                            for line in headers.lines() {
                                if line.to_lowercase().starts_with("content-type:")
                                    && let Some(cts) = line.split(':').nth(1)
                                    && let Some(ext) = guess_extension_from_content_type(cts.trim())
                                {
                                    final_dest = dest.with_extension(ext);
                                }
                            }
                            let _ = fs::remove_file(&headers_file);
                        } else {
                            let _ = fs::remove_file(&headers_file);
                        }
                        if let Err(e) = fs::rename(&staging, &final_dest) {
                            eprintln!(
                                "     [FAIL] Remote download rename failed for {}: {}",
                                s.path_or_url, e
                            );
                            let _ = fs::remove_file(&staging);
                            replica_fail += 1;
                            total_fail_other += 1;
                            continue;
                        }
                    }
                    _ => {
                        eprintln!("     [FAIL] curl download failed for {}", s.path_or_url);
                        let _ = fs::remove_file(&staging);
                        replica_fail += 1;
                        total_fail_other += 1;
                        continue;
                    }
                }
                replica_ok += 1;
            } else {
                if let Err(e) = fs::copy(&s.path_or_url, &dest) {
                    eprintln!(
                        "     [FAIL] Replica creation failed for {}: {}",
                        s.path_or_url, e
                    );
                    replica_fail += 1;
                    total_fail_other += 1;
                    continue;
                }
                replica_ok += 1;
            }

            // Use final_dest for all operations (same as dest for local files)
            let ingest_path = &final_dest;
            let filter_map = s.source.file_quality_filter.as_ref().map(|m| {
                let mut map = serde_json::Map::new();
                for (k, v) in m {
                    map.insert(k.clone(), v.clone());
                }
                map
            });
            if !passes_file_quality_filter(ingest_path, filter_map.as_ref()) {
                _total_skip += 1;
                continue;
            }

            if s.base_label == "animated_loop" {
                saw_loop_samples = true;
                loop_paths.push(final_dest);
                continue;
            }

            let raw_scenarios = get_scenarios(ingest_path, args.plan.no_loop);
            let scenarios = filter_scenarios(raw_scenarios, args.training_mode);
            if scenarios.is_empty() {
                _total_skip += 1;
                continue;
            }

            for scenario in scenarios {
                if scenario == "image_quality" {
                    saw_image_quality = true;
                }
                quality_tasks
                    .entry((s.base_label.clone(), scenario))
                    .or_default()
                    .push(final_dest.clone());
            }
        }
        eprintln!("     [PHASE] 1/2 done replicas_ok={replica_ok} fail={replica_fail}");

        eprintln!(
            "     [PHASE] 2/2 feature extraction + DB ingest (groups={}, loop_paths={})",
            quality_tasks.len(),
            loop_paths.len()
        );
        for ((label, scenario), paths) in quality_tasks {
            let (ok, fail_o, fail_lc) = ingest_quality_group(
                &paths,
                &label,
                &scenario,
                args.misc.verbose,
                args.exec.use_api,
                &conn_str,
            )?;
            total_success += ok;
            total_fail_other += fail_o;
            total_fail_label_conflict += fail_lc;
        }

        let (loop_ok, loop_fail_o, loop_fail_lc) = ingest_loop_group(
            &loop_paths,
            &args.loop_intent_label,
            args.misc.verbose,
            args.exec.use_api,
            &conn_str,
        )?;
        total_success += loop_ok;
        total_fail_other += loop_fail_o;
        total_fail_label_conflict += loop_fail_lc;
    }

    if args.effective_fill_runtime_assets() {
        let code = fill_runtime_assets(
            &conn_str,
            FillAssetsConfig {
                state: crate::run_training::types::AssetsState {
                    saw_image_quality,
                    saw_loop_samples,
                },
                actions: crate::run_training::types::AssetsActions {
                    install_deps: args.misc.install_missing_python_deps,
                    verify_after: args.assets.verify_after,
                },
                training_mode: args.training_mode,
            },
        )?;
        if code != 0 {
            eprintln!("  [WARN] Runtime assets finalization exited with code {code}");
        }
        return Ok((total_success, total_fail_other, total_fail_label_conflict));
    }

    if args.finalize.finalize_loop_intent && args.finalize.finalize_image_quality_model {
        let loop_exit = finalize_loop_intent_assets(&conn_str)?;
        let quality_exit =
            finalize_image_quality_model(&conn_str, args.misc.install_missing_python_deps)?;
        let code = i32::from(loop_exit != 0 || quality_exit != 0);
        if code != 0 {
            eprintln!("  [WARN] Partial finalize exited with code {code}");
        }
        return Ok((total_success, total_fail_other, total_fail_label_conflict));
    }
    if args.finalize.finalize_loop_intent {
        let code = finalize_loop_intent_assets(&conn_str)?;
        if code != 0 {
            eprintln!("  [WARN] finalize_loop_intent exited with code {code}");
        }
    } else if args.finalize.finalize_image_quality_model {
        if !saw_image_quality {
            eprintln!(
                "  [INFO] No new image_quality samples were routed in this run; \
                 checking existing corpus state before finalize."
            );
        }
        let code = finalize_image_quality_model(&conn_str, args.misc.install_missing_python_deps)?;
        if code != 0 {
            eprintln!("  [WARN] finalize_image_quality_model exited with code {code}");
        }
    }

    Ok((total_success, total_fail_other, total_fail_label_conflict))
}

#![allow(unused_imports, clippy::too_many_lines)]

use foundation::{
    log_anomaly, log_corruption, log_debug, log_detail, log_failure, log_fatal, log_hint,
    log_ignore, log_info, log_skip, log_success,
};

use std::path::Path;
use walkdir::WalkDir;

fn main() {
    log_detail!("Running test...");
    manual_debug_scan_debug_dir_only();
    log_detail!(foundation::infra::static_logs::messages::VERIFICATION_COMPLETE);
}

// Manual, header-only debug scanner for local `debug/` media.
//
// Safety: This test is gated behind the `MFB_RUN_DEBUG_SCAN` environment
// variable and will be skipped by default. It only performs cheap, header-
// level reads using the crate's `scan_gif_headers()` helper and will never
// invoke ffmpeg/ffprobe or mutate files. Limit defaults to avoid UI hangs.

fn manual_debug_scan_debug_dir_only() {
    // Disabled by default to avoid accidental scans of private media.
    if std::env::var("MFB_RUN_DEBUG_SCAN").is_err() {
        log_detail!(
            "Skipped manual debug scan. To run set MFB_RUN_DEBUG_SCAN=1 and optionally MFB_DEBUG_DIR=debug/media",
        );
        return;
    }

    let debug_dir = std::env::var("MFB_DEBUG_DIR").unwrap_or_else(|_| "debug/media".into());
    let root = Path::new(&debug_dir);
    if !root.exists() {
        log_detail!(
            "Debug path {} not found; set MFB_DEBUG_DIR to your local debug dir.",
            root.display(),
        );
        return;
    }

    let mut scanned = 0usize;
    // Safety: cap number of files scanned to avoid long-running runs.
    let cap = match std::env::var("MFB_DEBUG_SCAN_LIMIT") {
        Ok(raw) => raw.parse::<usize>().unwrap_or_else(|err| {
            log_detail!("Invalid MFB_DEBUG_SCAN_LIMIT={raw:?}: {err}; using 30");
            30usize
        }),
        Err(_) => 30usize,
    };

    let mut undecided: Vec<std::path::PathBuf> = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(core::result::Result::ok)
    {
        if scanned >= cap {
            break;
        }
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if p.extension()
            .and_then(|s| s.to_str())
            .map(str::to_lowercase)
            != Some("gif".to_string())
        {
            continue;
        }
        scanned += 1;
        log_detail!("--- Scanning: {}", p.display());

        match foundation::loop_intent::LoopMeta::from_gif_path(p) {
            Some(meta) => {
                let verdict = foundation::identify_loop_intent(&meta);
                let verd_str = format!("{verdict:?}");

                log_detail!(
                    "size={} palette={:?} loop={:?} alpha={} payload_var={:?} verdict={}",
                    meta.file_size_bytes,
                    meta.palette_size,
                    meta.loop_count,
                    meta.flags.streams.has_transparency,
                    meta.frame_payload_variation,
                    verd_str,
                );

                if matches!(verdict, foundation::LoopIntentVerdict::LoopStrong(_)) {
                    // Definitive: keep as GIF
                } else if matches!(verdict, foundation::LoopIntentVerdict::LoopWeak(_)) {
                    // Definitive: convert to video
                } else {
                    undecided.push(p.to_path_buf());
                }
            }
            None => {
                log_detail!("LoopMeta::from_gif_path failed for {}", p.display());
            }
        }
    }

    log_detail!("Scanned {scanned} GIF(s) (limit {cap}).");

    // Optional deeper sampling run: pick a small random subset of UNDECIDED
    // files and run the library's `should_keep_as_gif_with_path` for a
    // non-invasive, header+probe check. Trigger with MFB_RUN_DEEP_CHECK=1.
    if std::env::var("MFB_RUN_DEEP_CHECK").is_ok() && !undecided.is_empty() {
        let sample_count = match std::env::var("MFB_DEEP_SAMPLE_COUNT") {
            Ok(raw) => raw.parse::<usize>().unwrap_or_else(|err| {
                log_detail!("Invalid MFB_DEEP_SAMPLE_COUNT={raw:?}: {err}; using 5");
                5usize
            }),
            Err(_) => 5usize,
        }
        .min(undecided.len());
        log_detail!("Deep-checking {sample_count} random UNDECIDED sample(s)");

        // Simple linear-congruential generator for deterministic-ish sampling
        let mut seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or_else(
                |err| {
                    log_detail!("System clock before UNIX_EPOCH; using fixed sampling seed: {err}");
                    0xA5A5_5A5A_D3C3_B4B4
                },
                |d| {
                    foundation::numeric_cast::u128_low64_to_u64(
                        d.as_nanos() & 0xFFFF_FFFF_FFFF_FFFF,
                    )
                },
            );
        let undecided_len =
            u64::try_from(undecided.len()).expect("undecided length must fit into u64");
        let mut picks: Vec<usize> = Vec::new();
        while picks.len() < sample_count {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let idx = usize::try_from(seed % undecided_len)
                .expect("sample index is modulo vector length and must fit usize");
            if !picks.contains(&idx) {
                picks.push(idx);
            }
        }

        for idx in picks {
            let p = undecided
                .get(idx)
                .unwrap_or_else(|| panic!("missing index {idx}"));
            log_detail!("--- Deep sample: {}", p.display());
            if let Some(meta) = foundation::loop_intent::LoopMeta::from_gif_path(p) {
                let verdict = foundation::assess_loop_intent_from_meta(&meta, Some(p));
                log_detail!(
                    "Deep verdict: {:?} (duration={},size={})",
                    verdict,
                    meta.duration_secs
                        .map_or_else(|| "Unknown".to_string(), |d| format!("{d:.2}s")),
                    meta.file_size_bytes,
                );
            } else {
                log_detail!("Deep probe failed for {}", p.display());
            }
        }
    }
}

use std::path::Path;
use walkdir::WalkDir;

fn main() {
    println!("Running test...");
    manual_debug_scan_debug_dir_only();
    println!("✅ Test completed!");
}

// Manual, header-only debug scanner for local `debug/` media.
//
// Safety: This test is gated behind the `MFB_RUN_DEBUG_SCAN` environment
// variable and will be skipped by default. It only performs cheap, header-
// level reads using the crate's `scan_gif_headers()` helper and will never
// invoke ffmpeg/ffprobe or mutate files. Limit defaults to avoid UI hangs.

#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn manual_debug_scan_debug_dir_only() {
    // Disabled by default to avoid accidental scans of private media.
    if std::env::var("MFB_RUN_DEBUG_SCAN").is_err() {
        eprintln!(
            "Skipped manual debug scan. To run set MFB_RUN_DEBUG_SCAN=1 and optionally MFB_DEBUG_DIR=debug/media"
        );
        return;
    }

    let debug_dir = std::env::var("MFB_DEBUG_DIR").unwrap_or_else(|_| "debug/media".into());
    let root = Path::new(&debug_dir);
    if !root.exists() {
        eprintln!(
            "Debug path {} not found; set MFB_DEBUG_DIR to your local debug dir.",
            root.display()
        );
        return;
    }

    let mut scanned = 0usize;
    // Safety: cap number of files scanned to avoid long-running runs.
    let cap = std::env::var("MFB_DEBUG_SCAN_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(30usize);

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
        eprintln!("--- Scanning: {}", p.display());

        match shared_utils::loop_intent::LoopMeta::from_gif_path(p) {
            Some(meta) => {
                let verdict = shared_utils::identify_loop_intent(&meta);
                let verd_str = format!("{verdict:?}");

                eprintln!(
                    "size={} palette={:?} loop={:?} alpha={} payload_var={:?} verdict={}",
                    meta.file_size_bytes,
                    meta.palette_size,
                    meta.loop_count,
                    meta.flags.streams.has_transparency,
                    meta.frame_payload_variation,
                    verd_str
                );

                if matches!(verdict, shared_utils::LoopIntentVerdict::LoopStrong(_)) {
                    // Definitive: keep as GIF
                } else if matches!(verdict, shared_utils::LoopIntentVerdict::LoopWeak(_)) {
                    // Definitive: convert to video
                } else {
                    undecided.push(p.to_path_buf());
                }
            }
            None => {
                eprintln!("LoopMeta::from_gif_path failed for {}", p.display());
            }
        }
    }

    eprintln!("Scanned {scanned} GIF(s) (limit {cap}).");

    // Optional deeper sampling run: pick a small random subset of UNDECIDED
    // files and run the library's `should_keep_as_gif_with_path` for a
    // non-invasive, header+probe check. Trigger with MFB_RUN_DEEP_CHECK=1.
    if std::env::var("MFB_RUN_DEEP_CHECK").is_ok() && !undecided.is_empty() {
        let sample_count = std::env::var("MFB_DEEP_SAMPLE_COUNT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(5usize)
            .min(undecided.len());
        eprintln!("Deep-checking {sample_count} random UNDECIDED sample(s)");

        // Simple linear-congruential generator for deterministic-ish sampling
        let mut seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or_else(
                |err| {
                    eprintln!("System clock before UNIX_EPOCH; using fixed sampling seed: {err}");
                    0xA5A5_5A5A_D3C3_B4B4
                },
                |d| (d.as_nanos() & 0xFFFF_FFFF_FFFF_FFFF) as u64,
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
            eprintln!("--- Deep sample: {}", p.display());
            if let Some(meta) = shared_utils::loop_intent::LoopMeta::from_gif_path(p) {
                let verdict = shared_utils::assess_loop_intent_from_meta(&meta, Some(p));
                eprintln!(
                    "Deep verdict: {:?} (duration={:.2}s,size={})",
                    verdict,
                    meta.duration_secs.unwrap_or(0.0),
                    meta.file_size_bytes
                );
            } else {
                eprintln!("Deep probe failed for {}", p.display());
            }
        }
    }
}

// Manual, header-only debug scanner for local `debug/` media.
//
// Safety: This test is gated behind the `MFB_RUN_DEBUG_SCAN` environment
// variable and will be skipped by default. It only performs cheap, header-
// level reads using the crate's `scan_gif_headers()` helper and will never
// invoke ffmpeg/ffprobe or mutate files. Limit defaults to avoid UI hangs.

use walkdir::WalkDir;

#[test]
#[allow(clippy::too_many_lines, reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead.")]
fn manual_debug_scan_debug_dir_only() {
    // Disabled by default to avoid accidental scans of private media.
    if std::env::var("MFB_RUN_DEBUG_SCAN").is_err() {
        eprintln!(
            "Skipped manual debug scan. To run set MFB_RUN_DEBUG_SCAN=1 and optionally MFB_DEBUG_DIR=debug/media"
        );
        return;
    }

    let debug_dir = std::env::var("MFB_DEBUG_DIR").unwrap_or_else(|_| "debug/media".into());
    let root = std::path::Path::new(&debug_dir);
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
                let verdict = shared_utils::loop_intent::identify_loop_intent(&meta);
                let verd_str = format!("{verdict:?}");

                eprintln!(
                    "size={} palette={:?} loop={:?} alpha={} payload_var={:?} verdict={}",
                    meta.file_size_bytes,
                    meta.palette_size,
                    meta.loop_count,
                    meta.has_transparency,
                    meta.frame_payload_variation,
                    verd_str
                );

                if matches!(
                    verdict,
                    shared_utils::loop_intent::LoopIntentVerdict::LoopStrong(_)
                ) {
                    // Definitive: keep as GIF
                } else if matches!(
                    verdict,
                    shared_utils::loop_intent::LoopIntentVerdict::LoopWeak(_)
                ) {
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
            .map_or(0, |d| (d.as_nanos() & 0xFFFF_FFFF_FFFF_FFFF) as u64);
        let mut picks: Vec<usize> = Vec::new();
        while picks.len() < sample_count {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let idx = usize::try_from(seed).unwrap_or(0) % undecided.len();
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
                let verdict =
                    shared_utils::loop_intent::assess_loop_intent_from_meta(&meta, Some(p));
                eprintln!(
                    "Deep verdict: {:?} (duration={:.2}s,size={})",
                    verdict, meta.duration_secs, meta.file_size_bytes
                );
            } else {
                eprintln!("Deep probe failed for {}", p.display());
            }
        }
    }
}

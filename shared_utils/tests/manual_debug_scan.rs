// Manual, header-only debug scanner for local `debug/` media.
//
// Safety: This test is gated behind the `MFB_RUN_DEBUG_SCAN` environment
// variable and will be skipped by default. It only performs cheap, header-
// level reads using the crate's `scan_gif_headers()` helper and will never
// invoke ffmpeg/ffprobe or mutate files. Limit defaults to avoid UI hangs.

use std::io::Read;
use walkdir::WalkDir;

#[test]
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
        eprintln!("Debug path {} not found; set MFB_DEBUG_DIR to your local debug dir.", root.display());
        return;
    }

    let mut scanned = 0usize;
    // Safety: cap number of files scanned to avoid long-running runs.
    let cap = std::env::var("MFB_DEBUG_SCAN_LIMIT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(30usize);

    let mut undecided: Vec<std::path::PathBuf> = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if scanned >= cap {
            break;
        }
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if p.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()) != Some("gif".to_string()) {
            continue;
        }
        scanned += 1;
        eprintln!("--- Scanning: {}", p.display());

        match shared_utils::gif_meme_score::scan_gif_headers(p) {
            Ok((palette, exts, has_alpha, payload_var, delay_var, loop_count, total_dur)) => {
                // Read logical screen descriptor for width/height (bytes 6-9 little-endian)
                let mut f = match std::fs::File::open(p) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("open failed: {}", e);
                        continue;
                    }
                };
                let mut head = [0u8; 10];
                if f.read_exact(&mut head).is_err() {
                    eprintln!("failed to read header for {}", p.display());
                    continue;
                }
                let width = u16::from_le_bytes([head[6], head[7]]) as u32;
                let height = u16::from_le_bytes([head[8], head[9]]) as u32;

                let file_size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);

                // Best-effort frame count: count image descriptor 0x2C occurrences.
                let buf = std::fs::read(p).unwrap_or_default();
                let frame_count = buf.iter().filter(|&&b| b == 0x2C).count() as u64;

                let duration_secs = total_dur.unwrap_or((frame_count.max(1) as f64) / 12.0);
                let meta = shared_utils::gif_meme_score::GifMeta {
                    duration_secs,
                    width,
                    height,
                    fps: 12.0,
                    frame_count: frame_count.max(1),
                    file_size_bytes: file_size,
                    file_name: p.file_name().and_then(|s| s.to_str()).map(|s| s.to_string()),
                    palette_size: palette,
                    app_extensions: exts.clone(),
                    has_transparency: has_alpha,
                    frame_payload_variation: payload_var,
                    frame_delay_variation: delay_var,
                    source_extension: Some("gif".to_string()),
                    parent_directories: p.parent().map(|parent| {
                        parent
                            .iter()
                            .filter_map(|s| s.to_str().map(|x| x.to_string()))
                            .collect::<Vec<_>>()
                    }),
                    has_embedded_icc: false,
                    has_complex_color_profile: false,
                    loop_count,
                    has_audio: false,
                    frame_types: vec!['P'; frame_count.max(1) as usize],
                    pts_deltas: vec![1.0 / 12.0; frame_count.max(1) as usize],
                    mv_magnitudes: Vec::new(),
                    palette_depth: None,
                    motion_gini: None,
                    block_skew: None,
                    temporal_flatness: None,
                    pkt_sizes: vec![file_size / frame_count.max(1)],
                };

                let pixels = (u64::from(meta.width) * u64::from(meta.height)).max(1);
                let total_frames = meta.frame_count.max(1);
                let spatial_bpp = meta.file_size_bytes as f64 / (pixels as f64);
                let temporal_bpp = meta.file_size_bytes as f64 / (pixels as f64 * total_frames as f64);

                // Header-only conservative verdict (avoid calling private helpers
                // or ffmpeg). This mirrors the early ABSOLUTE guards and a
                // short-loop header-scan guard used by the library.
                                // Local copies of a few library thresholds so this test remains
                                // header-only and doesn't need access to private symbols.
                                const ABSOLUTE_KEEP_UNDER_BYTES: u64 = 102_400; // 100 KiB
                                const ABSOLUTE_CONVERT_OVER_BYTES: u64 = 50_000_000; // 50 MiB

                                fn dynamic_duration_threshold_local(width: u32, height: u32, fps: f64) -> f64 {
                                    const T_REF: f64 = 4.25;
                                    const N_REF: f64 = 518_400.0; // 720×720
                                    const FPS_REF: f64 = 12.0;
                                    const GAMMA: f64 = 0.30;

                                    let n = (f64::from(width) * f64::from(height)).max(1.0);
                                    let fps_clamped = fps.clamp(3.0, 60.0);
                                    let fps_factor = (FPS_REF / fps_clamped).powf(GAMMA);
                                    (T_REF * (N_REF / n).sqrt() * fps_factor.clamp(0.65, 1.60)).max(0.3)
                                }

                                fn convert_ceiling_local(width: u32, height: u32) -> u64 {
                                    let n = u64::from(width) * u64::from(height);
                                    n * 8u64
                                }

                                let mut verdict_str = "UNDECIDED".to_string();
                                if meta.file_size_bytes <= ABSOLUTE_KEEP_UNDER_BYTES {
                    verdict_str = "KEEP (absolute small)".to_string();
                } else if meta.file_size_bytes >= ABSOLUTE_CONVERT_OVER_BYTES {
                    verdict_str = "CONVERT (absolute large)".to_string();
                } else {
                    // Short-loop header guard (infinite loop OR low payload-variation)
                                    let dyn_thr = dynamic_duration_threshold_local(meta.width, meta.height, meta.fps).max(0.35);
                    let shortish = meta.duration_secs <= dyn_thr * 1.6;
                    let low_variation = meta.frame_payload_variation.unwrap_or(1.0) < 0.18;
                                    let small_size_guard = meta.file_size_bytes <= convert_ceiling_local(meta.width, meta.height) / 2;
                    if (meta.loop_count == Some(0) || low_variation) && shortish && small_size_guard {
                        verdict_str = "KEEP (short-loop header guard)".to_string();
                    } else if meta.has_transparency {
                        verdict_str = "KEEP (transparency)".to_string();
                    }
                }

                eprintln!(
                    "size={} palette={:?} loop={:?} alpha={} payload_var={:?} verdict={}",
                    meta.file_size_bytes,
                    meta.palette_size,
                    meta.loop_count,
                    meta.has_transparency,
                    meta.frame_payload_variation,
                    verdict_str
                );
                if verdict_str.starts_with("UNDECIDED") {
                    undecided.push(p.to_path_buf());
                }
            }
            Err(e) => {
                eprintln!("scan_gif_headers failed: {}", e);
            }
        }
    }

    eprintln!("Scanned {} GIF(s) (limit {}).", scanned, cap);

    // Optional deeper sampling run: pick a small random subset of UNDECIDED
    // files and run the library's `should_keep_as_gif_with_path` for a
    // non-invasive, header+probe check. Trigger with MFB_RUN_DEEP_CHECK=1.
    if std::env::var("MFB_RUN_DEEP_CHECK").is_ok() && !undecided.is_empty() {
        let sample_count = std::env::var("MFB_DEEP_SAMPLE_COUNT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(5usize)
            .min(undecided.len());
        eprintln!("Deep-checking {} random UNDECIDED sample(s)", sample_count);

        // Simple linear-congruential generator for deterministic-ish sampling
        let mut seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut picks: Vec<usize> = Vec::new();
        while picks.len() < sample_count {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let idx = (seed as usize) % undecided.len();
            if !picks.contains(&idx) {
                picks.push(idx);
            }
        }

        for idx in picks {
            let p = &undecided[idx];
            eprintln!("--- Deep sample: {}", p.display());
            if let Some(meta) = shared_utils::gif_meme_score::gif_candidate_meta_from_path(p) {
                let keep = shared_utils::gif_meme_score::should_keep_as_gif_with_path(&meta, Some(p));
                eprintln!("Deep verdict: {} (duration={:.2}s,size={})", if keep {"KEEP"} else {"CONVERT"}, meta.duration_secs, meta.file_size_bytes);
            } else {
                eprintln!("Deep probe failed for {}", p.display());
            }
        }
    }
}

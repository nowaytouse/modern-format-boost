# Release Notes (for GitHub Release page)

**Version:** 8.5.0  
**Date:** 2026-02-24

---

## Four binaries / 四个可执行文件

| Binary | Purpose |
|--------|--------|
| **img-hevc** | Image quality analysis and conversion to JXL/HEIC (HEVC path). |
| **img-av1** | Image quality analysis and conversion to JXL/AVIF (AV1 path). |
| **vid-hevc** | Video analysis and conversion to HEVC/H.265. |
| **vid-av1** | Video analysis and conversion to AV1. |

After building with `cargo build --release`, they are under `target/release/` as `img-hevc`, `img-av1`, `vid-hevc`, `vid-av1`.  
For a GitHub Release, attach these four binaries from that directory.

---

## Recent changes (v8.5.0 and audit fixes)

### Correctness & robustness
- **GIF parser**: Bounds check in `count_frames_from_bytes` to avoid panic on truncated or malformed GIF.
- **Processed list**: Advisory file lock (Unix `flock`) on load/save to prevent corruption when multiple processes use the same list.
- **rsync**: Path is now resolved via `which::which("rsync")` instead of hardcoded Homebrew paths.
- **Path safety**: All external command (ffmpeg, ffprobe, cjxl, etc.) arguments use `safe_path_arg` to avoid path-injection and non-UTF-8 issues.
- **Div-by-zero**: Guards in stream_analysis, gpu_coarse_search, conversion APIs, and image size_reduction.
- **GPU**: Configurable concurrency limit (`MODERN_FORMAT_BOOST_GPU_CONCURRENCY`); VAAPI device path configurable via env.

### Logging & UX
- Per-file log prefix `[filename]` to avoid interleaved output in parallel runs.
- Stderr indentation and fixed-width tag column; ANSI stripped when not TTY and in log files.
- ImageMagick fallback for WebP/GIF animation duration when ffprobe has none.
- GIF quality verification: SSIM path with format normalization and alpha flatten; clearer “N/A” when verification is skipped.

### Quality & pipeline
- **quality_verifier_enhanced**: Output health check and optional duration/stream checks after encode.
- **heartbeat_manager**: Atomic underflow fix in unregister.
- **x265 pipeline**: Broken-pipe handling and clearer decoder/encoder failure messages.

### Docs & code
- README: Processing flow summary (EN+ZH) and CLI examples for all four tools.
- CODE_AUDIT.md: §30 for GIF/rsync/processed-list fixes; summary table updated.

---

## How to build

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern_format_boost
cargo build --release
# Binaries: target/release/img-hevc, img-av1, vid-hevc, vid-av1
```

Requirements: `jpeg-xl`, `ffmpeg`, `imagemagick`, `exiftool` (e.g. `brew install jpeg-xl ffmpeg imagemagick exiftool` on macOS).

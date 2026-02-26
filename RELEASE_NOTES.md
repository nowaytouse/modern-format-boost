# Release Notes (for GitHub Release page)

**Version:** 8.7.0
**Date:** 2026-02-27

---

## Highlights

### Critical Bug Fixes & Robustness
- **GIF Quality Verification**: Removed unsafe SSIM-only fallback for GIF files. Now explicitly fails with clear error message instead of false success, preventing potential quality loss.
- **Single-File Copy-on-Fail**: When converting a single file with `--output` fails, the original file is now copied to the output directory before returning error, ensuring no data loss.
- **Calibration stderr Diagnostics**: FFmpeg stderr is now printed in full when calibration fails, aiding troubleshooting.
- **TOCTOU-Safe Conversion**: Implemented temp file + atomic rename pattern in conversion APIs to prevent time-of-check-time-of-use race conditions.

### Apple Ecosystem Improvements
- **Script Behavior**: Disabled automatic Apple Photos Compatibility Repair run; user confirmation required before processing.
- **JXL Metadata**: Strip metadata only on grayscale+ICC retry path; preserved metadata documented.
- **Extension Correction**: Fixed extension mismatch handling to prevent format confusion (e.g., animated GIF/WebP not confused with video).

### Code Quality & Audit
- **Comprehensive Audit**: Completed CODE_AUDIT.md with 39+ sections covering path safety, concurrency, div-by-zero, and panic analysis.
- **Dependency Updates**: Bumped `libheif-rs` to 2.6.1, `tempfile` to 3.26.
- **Logging Unification**: All user-facing errors and logs unified to English; ANSI stripped for non-TTY and log files.

### Logging & UX
- **Per-File Log Context**: Parallel file processing now prefixes each line with `[filename]` for clear attribution.
- **Progress Display**: Images OK/failed displayed on same line as XMP/JXL milestones for compact output.
- **Ultimate Mode**: Extended MS-SSIM skip threshold to 25 minutes for long-form video optimization.

---

**Version:** 8.6.0
**Date:** 2026-02-24

---

## Highlights

### Comprehensive Audit & Robustness
- **Safety**: Fixed multiple potential divide-by-zero errors in stream analysis, GPU search, and image size reduction logic.
- **Path Safety**: Enhanced argument sanitization for external commands (ffmpeg, ffprobe, cjxl) to prevent path injection and handle non-UTF-8 paths correctly.
- **Concurrency**: Configurable GPU concurrency limit (`MODERN_FORMAT_BOOST_GPU_CONCURRENCY`) and VAAPI device path.
- **Pipeline**: Improved pipe error handling for x265/ffmpeg to avoid deadlocks and provide clearer error messages.
- **Logic**: Refined logic in `video_explorer`, `explore_strategy`, and `image_metrics` (SSIM/MS-SSIM correctness).
- **Domain Wall**: Adjusted "Ultimate mode" domain wall to require 15-20 zero-gain attempts, improving convergence.

### Logging & UX
- **Clean Logging**: Strip ANSI color codes when outputting to files or non-TTY environments.
- **Unified Output**: Standardized log prefixes and indentation across all modules for better readability in parallel execution.

### Fixes
- **Img**: Unified compression checks for all image conversion paths.
- **Video**: Corrected codec detection and GIF verification logic.

---

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

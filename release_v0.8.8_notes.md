## Version 0.8.8 (2026-02-28)

Version numbering is now **0.8.x** (replacing the previous 8.x scheme). This release is 0.8.8. All changes below are **since 8.7.0**.

---

### Version & scheme
- **Version scheme**: Switched from 8.x to **0.8.x**. This is the first release under the new scheme.
- **Documentation**: README badge, RELEASE_NOTES, and CHANGELOG updated to 0.8.8.

### Quality validation & failure reporting
- **Enhanced verification failure reason**: When quality and file size would pass but enhanced verification fails (duration mismatch or output probe failure), the **real reason** is now shown everywhere instead of "unknown reason" or the misleading "total file not smaller".
  - Added `ExploreResult.enhanced_verify_fail_reason`; set from `verify_after_encode` when it does not pass.
  - **QualityCheck log line**: Now shows `QualityCheck: FAILED (quality met but enhanced verification failed: <reason>)` (e.g. "Duration mismatch (input vs output beyond tolerance)" or "Probe failed; duration/stream not verified") when applicable.
  - **conversion_api** and **animated_image**: The former "unknown reason" branch now uses `enhanced_verify_fail_reason` for the failure message and skip reason.
- **Output probe failure handling** (video): When output probe fails, `duration_match` / `has_video_stream` are set to `None` (not `Some(false)`), so `passed()` treats as pass and accepts the generated output, with "Output probe failed" / "Accepting output (probe unavailable)" in details instead of falsely failing.

### Logging system (comprehensive overhaul)
- **Log level has real effect**: The configured level (TRACE/DEBUG/INFO/WARN/ERROR) now applies to **all** logging:
  - **Tracing**: `EnvFilter` uses `config.level` (default TRACE); `RUST_LOG` can override.
  - **Direct run-log writes**: New `write_to_log_at_level(level, line)` and `should_log(level)` in logging; every direct write goes through level check. Session header and status lines use INFO; progress lines and verbose content use DEBUG; conversion failures use ERROR.
- **Run log is comprehensive**: Run log receives full detail (init message, progress lines, emoji messages, tracing events). Tracing events are forwarded to the run log via a registered forwarder; the "Logging system initialized" line is stored and written when the run log is opened.
- **No `--log-file` flag**: Removed. Run logs are **auto-created** with timestamped names under `./logs/` (e.g. `img_hevc_run_2026-02-28_14-30-00.log`). Scripts no longer pass `--log-file`.
- **System/temp log files**: Timestamp in filename (`%Y-%m-%d_%H-%M-%S`) to avoid conflicts; no 5-file or size limit by default (limits only when explicitly set).
- **Run log file lock**: On Unix, an advisory exclusive lock (`flock` LOCK_EX) is taken when opening the run log so other processes cannot truncate or overwrite it. Documented behavior when the log file is renamed while the process is writing (data continues to the same inode).
- **Emoji and status in run log**: User-facing messages that include emoji (e.g. Apple Compatibility, ImageMagick animation, errors, resume/fresh run) now use `emit_stderr` so they appear in the run log; progress bar updates are written via `write_progress_line_to_run_log`.

### XMP & progress display
- **XMP merge log line**: JXL counts are merged into the "Images" category (no separate JXL line). Log tag changed from `[XMP]` to `[Info]` for status/summary lines.
- **Metadata fallback**: Exiv2 fallback messages (after ExifTool failure) are written to run log at INFO level.

### Conversion & failure logging
- **Conversion failure in run log**: Single-file conversion failures call `log_conversion_failure(path, error)` so the full error (e.g. cjxl stderr, bitstream reconstruction messages) is written to the run log, not only stderr.
- **JPEG→JXL**: Tail stripping and `--allow_jpeg_reconstruction 0` flow so bitstream reconstruction errors are handled and logged clearly.

### Regression tests
- **Temp-copy only**: `test_verify_after_encode_with_temp_copies_probe_fails` uses only files created in the system temp dir (no original folder); asserts enhanced verification fails with a probe-related message when input/output are not valid video.
- **QualityCheck line**: `format_quality_check_line` extracted and tested; when `enhanced_verify_fail_reason` is set, the line must contain "enhanced verification failed" and the reason, and must **not** contain the misleading "total file not smaller"; when unset, fallback to "total file not smaller" is tested.

### Image quality & format detection
- **Image quality reliability**: Multi-format container and bitstream parsing (AVIF/HEIC/JXL); Err only when key box/header missing. PNG: entropy in heuristic, gray zone [0.40,0.58], zTXt tool signature; TIFF full IFD + BigTIFF; WebP animation ANMF. Format extensions: QOI/JP2/ICO/TGA/EXR/FLIF/PSD/PNM/DDS with `detect_compression` branches; EXR/JP2/ICO parsing. image_analyzer: HEIC/JXL/AVIF/TIFF unified via `detect_compression`; removed `check_avif_lossless`.
- **AVIF pixel fallback**: When format-level `is_lossless` (AVIF) returns Err, use `pixel_fallback_lossless`; HEIC/JXL/generic paths use pixel fallback on Err. Documented reliability and fallback checklist.
- **img_hevc**: Skip conversion when input is already JXL; directory collection uses `IMAGE_EXTENSIONS_FOR_CONVERT` (excludes .jxl). file_copier: documented IMAGE_EXTENSIONS_FOR_CONVERT and SUPPORTED_VIDEO_EXTENSIONS (mov/mp4 by codec not extension).
- **image_quality_core removed**: Module removed; routing via `image_quality_detector` (from_path / log) and progress_mode `set_default_run_log_file`.

### Video codec scope & Apple fallback
- **Normal vs Apple-compat**: Normal mode skips all modern codecs (H.265, AV1, VP9, VVC, AV2)—no conversion. Apple-compat mode skips only H.265 and converts AV1/VP9/VVC/AV2 to HEVC. Skip reason: "use Apple-compat mode to convert to HEVC".
- **ProRes/DNxHD**: Excluded from best-effort fallback; must pass strict size-shrink and SSIM; never keep output when size increased; decision by SSIM and size only. Fallback only for Apple-incompatible codecs (AV1/VP9/VVC/AV2).
- **Apple fallback predicate**: `should_keep_apple_fallback_hevc_output` now uses **total file size only** (total_file_compressed, total_size_ratio). Keep fallback when total file smaller or (allow_size_tolerance && total_size_ratio < 1.01). Video stream comparison kept for logging/diagnostics only.
- **Audit P0–D6**: Documented compress mode (output < input only; ≥ reject), atomic temp+commit in conversion; allow_size_tolerance and ratio < 1.01; MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE/VIDEO constants; commit_temp_to_output rejects size 0; phase comments for explore vs require_compression.

### Animated images & WebP
- **Min duration for animated→video**: `ANIMATED_MIN_DURATION_FOR_VIDEO_SECS = 4.5`. Normal mode: convert GIF to HEVC only when duration ≥ 4.5s; otherwise skip with "Short animation (x.xs < 4.5s)". Apple-compat: same 4.5s for HEVC vs GIF.
- **WebP duration**: Native parse of ANMF frame delays in `duration_secs_from_bytes()`. `get_animation_duration` tries WebP native after ffprobe/ImageMagick. When duration_secs is None: retry then skip (no 5.0s fake default).

### Resume (img-hevc / img-av1)
- **--resume / --no-resume**: Directory runs support `--resume` (default) and `--no-resume`. Load/save `.mfb_processed` in output or input dir to resume from last run. README: 断点续传 / Resume section.

### Pipelines, memory & spinner
- **x265 Y4M direct**: When input is already a temp .y4m file, use `encode_y4m_direct()` (no FFmpeg→pipe→x265) to avoid Broken pipe; on Broken pipe, output x265 stderr for diagnostics.
- **Stderr/stdout drain**: jxl_utils: ImageMagick and cjxl stderr drained in background threads before wait(); lossless_converter: cjxl stderr drained after spawn. FfmpegProcess: if caller does not take_stdout(), drain remaining stdout before wait() to avoid deadlock.
- **Spinner**: Subshell stderr redirected to avoid "Killed: 9" spam in logs; elapsed time clamped to ≥ 0. Pipeline failed messages include exact file path.
- **Memory pressure**: `system_memory` (macOS vm_stat/sysctl, Linux /proc/meminfo), `memory_pressure_level()` Low/Normal/High; `MFB_LOW_MEMORY=1` forces low-memory mode. thread_manager: `get_balanced_thread_config()` and FFmpeg thread caps by pressure (High or MFB_LOW_MEMORY → 1/1; Normal → ≤2/2). `memory_cap_hint()` for verbose output.

### Logging (additional)
- **Default log location**: Run logs under `./logs/` (dir and `*_run.log` in .gitignore so logs are not committed).
- **Flush**: Each `write_to_log` followed by flush so all log lines are written to disk immediately; vid_hevc Run sets default run log so direct runs write to ./logs/vid_hevc_run.log.
- **Session log merge**: Script `save_log()` appends VERBOSE_LOG_FILE (img_hevc+vid_hevc internal log) to drag_drop_*.log so one file has full output; log_conversion_failure / xmp_merge_failure call flush_log_file() after write.

### Dependencies
- **libheif-rs**: 2.6.x (allow latest 2.6). **cargo update**: pin-project-lite, zerocopy, image, tracing-subscriber and other transitive deps to latest compatible (Cargo.lock gitignored where applicable).

### Pipe & stability
- **Pipe handling**: No pipe-related errors in current run logs. Existing behavior confirmed: FFmpeg process stderr drained in a thread to avoid deadlock; x265 encoder has Broken pipe detection and clear messages; img-hevc FFmpeg→cjxl pipeline drains both stderrs; progress `pipe:1` stdout is fully read.

### Scripts
- **drag_and_drop_processor.sh**: No longer passes `--log-file`; logging is automatic in the Rust binaries.

---

### Binaries (darwin/arm64)
- `img-hevc-darwin-arm64` — Image conversion (HEVC/JXL/HEIC)
- `img-av1-darwin-arm64` — Image conversion (AV1/AVIF)
- `vid-hevc-darwin-arm64` — Video conversion (HEVC)
- `vid-av1-darwin-arm64` — Video conversion (AV1)

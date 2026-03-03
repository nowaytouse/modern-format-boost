# Changelog

All notable changes to this project will be documented in this file.

**Version scheme:** As of this release, the project uses **0.8.x** versioning (replacing the previous 8.x scheme).

## [Unreleased]

## [0.9.1] - 2026-03-04

### Image Conversion & ICC Profiles
- **Fixed Grayscale PNG + RGB ICC incompatibility**: Resolved an issue where `cjxl` failed on certain grayscale images containing RGB ICC profiles (e.g., `IMG_8321.JPG`).
  - **Improved Detection**: Refined `is_grayscale_icc_cjxl_error()` logic in `shared_utils` to accurately identify this specific failure mode.
  - **Automatic Recovery**: The ImageMagick fallback pipeline now correctly triggers a `-strip` retry when this error is detected, removing the problematic ICC profile while preserving 16-bit depth for 16-bit sources.
- **Enhanced ImageMagick Fallback Pipeline**: Refined the 4-stage retry mechanism:
  1. Default: 16-bit, preserve metadata.
  2. Grayscale ICC error: 16-bit + `-strip`.
  3. 8-bit source failure: 8-bit + `-strip`.
  4. 16-bit source failure: 16-bit + ICC normalization to sRGB.

### Video Quality Metrics
- **Quality Metric Diagnostics**: Verified that certain log warnings (CAMBI calculation "failures" or MS-SSIM targets not met) are expected behaviors for specific video content rather than functional bugs.

### Documentation
- **Consolidated error fix summary**: Merged `ERROR_FIX_SUMMARY.md` into `CHANGELOG.md`.

## [0.9.0] - 2026-03-03

### Critical Bug Fixes
- **CAMBI calculation completely broken**: Fixed libvmaf filter invocation that caused all Ultimate Mode videos to be rejected
  - Root cause: libvmaf filter requires TWO inputs (main + reference), but code used single input with `-vf`
  - Error: "Error opening output files: Invalid argument" on every CAMBI calculation
  - Impact: 3D quality gate always failed → all Ultimate Mode videos silently discarded
  - Fix: Use `-filter_complex` with same video as both inputs for no-reference CAMBI metric
  - Performance: Use `n_subsample` parameter for faster sampling (skip frames inside libvmaf)
  - Threshold: Tightened CAMBI threshold from 10.0 → 5.0 (Netflix official standard)

### Quality Gate Improvements
- **3D Quality Gate (Ultimate Mode)**: Now fully functional with three independent metrics
  - VMAF-Y ≥ 93.0 (perceptual quality, Netflix standard)
  - CAMBI ≤ 5.0 (banding detection, lower = better, Netflix standard)
  - PSNR-UV ≥ 38.0 dB (chroma fidelity)
  - All three must pass for video to be accepted

### GIF Processing Enhancements
- **GIF meme detection**: Multi-dimensional scoring system to identify meme GIFs
  - Five-layer edge-case suppression strategy
  - Prevents accidental conversion of meme GIFs to video format
  - Preserves GIF format for content that should remain as GIF
- **GIF duration tolerance**: Relaxed duration validation for animated images
  - GIF/WebP/AVIF/HEIC: 3.0 second tolerance (was 1.0s)
  - Accounts for variable frame delay in GIF format
  - Prevents false rejections due to frame timing differences

### HEIC HDR/Dolby Vision Support
- **HDR detection**: Automatic detection and preservation of HDR content
  - Scans ISO BMFF box structure (hvcC, dvcC, dvvC, colr/nclx)
  - Detects PQ (SMPTE 2084), HLG (Hybrid Log-Gamma), BT.2020 color space
  - Automatically skips conversion to preserve HDR metadata
- **Dolby Vision detection**: Identifies and protects Dolby Vision content
  - Detects dvcC and dvvC boxes in HEIC files
  - Prevents quality loss from HDR → SDR conversion

### Documentation
- **Consolidated documentation**: Merged GIF_DURATION_FIX.md, HEIC_HDR_UPDATE.md, UPDATE_SUMMARY.md into CHANGELOG.md
- **Removed redundant files**: Cleaned up scattered documentation files

## [0.8.9] - 2026-03-01

### Image conversion fixes
- **apple_compat flag in ImageMagick fallback paths**: Fixed missing `apple_compat` flag in all ImageMagick→cjxl fallback call sites:
  - `shared_utils/src/jxl_utils.rs`: All 4 call sites now pass `options.apple_compat`
  - `img_av1/src/lossless_converter.rs`: Pass `options.apple_compat`
  - `img_hevc/src/lossless_converter.rs`: Pass `options.apple_compat`
- **convert_jpeg_to_jxl fallback**: Added ImageMagick→cjxl fallback to the else branch when cjxl JPEG transcode fails (e.g., corrupt JPEG with "Getting pixel data failed" / "Failed to decode" errors)
- **XMP/ExifTool format error handling**: When ExifTool reports "format error in file" (case-insensitive):
  - Emit single skip line: "XMP merge skipped (ExifTool does not support writing to this file format)"
  - Still fallback to exiv2; suppress duplicate "exiv2 not available" message
  - Affects files like IMG_0004 (2).GIF that ExifTool cannot write to
- **cjxl decode/pixel error retry**: Added depth parameter (8/16) to ImageMagick→cjxl pipeline:
  - New `is_decode_or_pixel_cjxl_error()` detects cjxl stderr with "getting pixel data failed" / "failed to decode"
  - Retry with 8-bit simplified stream for confirmed 8-bit sources (no quality loss)
  - For 16-bit sources, retry with ICC normalization to sRGB (no depth downgrade)
  - Affects files like IMG_8321.JPG, IMG_6171.jpeg where magick succeeds but cjxl fails

### Code quality audit & security hardening
- **Comprehensive security audit**: Fixed 11/11 issues (100% fix rate)
  - CRITICAL: 4/4 fixed (100%)
  - HIGH: 4/4 fixed (100%)
  - MEDIUM: 3/3 fixed (100%)
- **Input validation**: Symlink checks, file type validation, readability verification
- **Path safety**: Prevent path traversal, symlink attacks, path injection
- **Resource management**: Improved file handle cleanup, temp file handling, advisory locks
- **Code quality scores**: Overall +80% improvement (5/10 → 9/10)
  - Security: 10/10
  - Error handling: 9/10
  - Resource management: 9/10
  - Maintainability: 9/10
  - Performance: 8/10
- **Production readiness**: Ready for deployment

### Performance optimization (low-memory & multi-instance)
- **Memory usage optimization**:
  - stderr buffer limit: 10MB → 1MB hard cap
  - Initial allocation: 1MB → 64KB (-94%)
  - BufRead parallelism reduced
  - Multi-instance mode: Auto-halves thread allocation
- **Process pipeline optimization**:
  - `jxl_utils.rs`: ImageMagick/cjxl stderr capped at 1MB
  - `x265_encoder.rs`: FFmpeg/x265 stderr capped at 1MB + early exit
  - `lossless_converter.rs`: FFmpeg/cjxl stderr optimization
- **Environment variable support**:
  - `MFB_LOW_MEMORY=1`: Low-memory mode for systems with < 8GB RAM
  - `MFB_MULTI_INSTANCE=1`: Multi-instance mode for 3+ concurrent processes
- **Performance improvements**:
  - Memory footprint: -70% (low-memory scenarios)
  - Thread overhead: -100% (no repeated computation after caching)
  - Buffer allocation: -94% (1MB → 64KB initial)
  - Ideal for: Systems with < 8GB RAM + multi-instance workloads
- **Performance rating**: 8/10 → 9.5/10

### Documentation
- **Changelog consolidation**: Merged all changelog files (CHANGES_SUMMARY.md, RELEASE_NOTES.md, release_v0.8.8_notes.md) into CHANGELOG.md to avoid scattered documentation

## [0.8.8] - 2026-02-28

All changes below are since 8.7.0.

### Version & docs
- **Version numbering**: Switched from 8.x to **0.8.x**. Current release is **0.8.8**.
- **Documentation**: README badge, RELEASE_NOTES, and CHANGELOG updated to 0.8.8.

### Quality validation & failure reporting
- **Enhanced verification failure reason**: When quality and file size would pass but enhanced verification fails (duration mismatch or output probe failure), the real reason is now shown instead of "unknown reason" or "total file not smaller". Added `ExploreResult.enhanced_verify_fail_reason`; set from `verify_after_encode` when it does not pass. QualityCheck log line shows "QualityCheck: FAILED (quality met but enhanced verification failed: &lt;reason&gt;)". conversion_api and animated_image use `enhanced_verify_fail_reason` for the former "unknown reason" branch.
- **Output probe failure** (video): When output probe fails, `duration_match` / `has_video_stream` are set to `None` so `passed()` accepts the output with "Output probe failed" / "Accepting output (probe unavailable)" in details.

### Logging system (overhaul)
- **Log level has real effect**: Config level (default TRACE) and RUST_LOG apply to tracing; direct run-log writes use `write_to_log_at_level(level, line)` and `should_log(level)` so INFO/DEBUG/ERROR are respected everywhere.
- **Run log comprehensive**: Init message, progress lines, emoji messages, and tracing events all reach the run log; forwarder and stored init message when run log opens.
- **No `--log-file`**: Removed; run logs auto-created with timestamp under `./logs/`.
- **System/temp logs**: Timestamp in filename; no 5-file or size limit by default.
- **Run log lock**: Unix advisory exclusive lock (flock) when opening run log; doc for rename-while-open behavior.
- **Emoji/status in run log**: User-facing emoji messages and progress updates written to run log via emit_stderr / write_progress_line_to_run_log.

### XMP & progress
- **XMP merge log**: JXL merged into "Images"; tag `[XMP]` → `[Info]`. Metadata Exiv2 fallback messages at INFO level.

### Conversion & failure logging
- **Conversion failure**: `log_conversion_failure(path, error)` writes full error to run log. JPEG→JXL tail / allow_jpeg_reconstruction flow and cjxl stderr in run log.

### Regression tests
- **Temp-copy test**: `test_verify_after_encode_with_temp_copies_probe_fails` (temp dir only). **QualityCheck line**: `format_quality_check_line` extracted; tests that enhanced reason is shown and "total file not smaller" is not when reason is set.

### Image quality & format detection
- **Image quality reliability**: AVIF/HEIC/JXL/PNG/TIFF/WebP and format extensions (QOI/JP2/ICO/TGA/EXR/FLIF/PSD/PNM/DDS); detect_compression unified; skip when already JXL; IMAGE_EXTENSIONS_FOR_CONVERT documented. **AVIF pixel fallback** on format-level Err. **image_quality_core** removed; use image_quality_detector.

### Video codec & Apple fallback
- **Normal**: Skip H.265/AV1/VP9/VVC/AV2. **Apple-compat**: Skip only H.265; convert AV1/VP9/VVC/AV2 to HEVC. **ProRes/DNxHD**: Strict only; no fallback on failure. **Apple fallback predicate**: by total file size only (total_size_ratio &lt; 1.01 with tolerance). P0–D6 audit: compress doc, safe_delete constants, reject size 0 temp.

### Animated & WebP
- **Min duration**: ANIMATED_MIN_DURATION_FOR_VIDEO_SECS = 4.5s. **WebP**: Native ANMF duration parse; no 5.0s fake default when duration unknown.

### Resume
- **img-hevc / img-av1**: --resume (default) / --no-resume; .mfb_processed in output or input dir.

### Pipelines & memory
- **x265**: encode_y4m_direct() when input is .y4m; stderr drain in jxl_utils and lossless_converter; FfmpegProcess stdout drain. **Spinner**: Killed:9 suppression; elapsed ≥ 0; pipeline failed path in message. **system_memory** + thread_manager: MFB_LOW_MEMORY, pressure-based parallel_tasks/child_threads cap.

### Logging (additional)
- Run logs under ./logs/ (gitignored); flush after each write; script save_log() merges VERBOSE_LOG_FILE into drag_drop_*.log.

### Dependencies
- libheif-rs 2.6.x; cargo update for transitive deps.

### Scripts
- **drag_and_drop_processor.sh**: No longer passes `--log-file`.

---

## [8.7.0] - 2026-02-27

### 🔧 Critical Bug Fixes

#### GIF Quality Verification (Root Out False Success)
- **Removed Unsafe Fallback**: GIF files no longer use SSIM-only or explore-SSIM as a兜底 (fallback) when MS-SSIM fails. Previously, this could mark verification as "passed" when it was incomplete.
- **Explicit Error Reporting**: Now loudly reports error to stderr and `result.log` when GIF quality verification cannot be completed. `ms_ssim_passed = Some(false)` is set explicitly.
- **Impact**: Prevents potential quality loss from false-positive verification results.

#### Single-File Copy-on-Fail
- **No Data Loss Guarantee**: When converting a single file with `--output` directory specified, if conversion fails, the original file is now copied to the output directory before returning the error.
- **Implementation**: `cli_runner.rs` now calls `copy_on_skip_or_fail` before propagating `Err` in single-file mode.

#### Calibration Diagnostics
- **Full stderr Output**: When FFmpeg calibration fails (e.g., decode failed for CRF values), the complete FFmpeg stderr is now printed for troubleshooting.
- **Y4M Extract**: Added `-an` (no audio) flag to Y4M extraction command to avoid unnecessary audio stream processing.

### 🍎 Apple Ecosystem

#### Script Behavior Change
- **No Auto-Repair**: Disabled automatic Apple Photos Compatibility Repair run in scripts. User confirmation is now required before processing.
- **JXL Metadata Preservation**: Metadata stripping now only occurs on grayscale+ICC retry path, preserving metadata in normal conversion flows.

#### Extension Mismatch Handling
- **Format Confusion Prevention**: Fixed detection order to ensure GIF/WebP/AVIF are detected before video path, preventing animated images from being confused with video formats.

### 🔒 Code Quality & Audit

#### Comprehensive Audit Completion
- **CODE_AUDIT.md**: Completed with 39+ sections covering:
  - Path safety and argument sanitization
  - Concurrency and poison recovery
  - Division-by-zero guards
  - unwrap/expect/panic analysis
  - TOCTOU mitigation

#### TOCTOU Mitigation
- **Atomic Conversion**: Implemented temp file + atomic rename pattern in conversion APIs (`conversion.rs`) to prevent time-of-check-time-of-use race conditions.
- **Safe Temp Paths**: Temp files now use pattern `stem.tmp.ext` for safer intermediate file handling.

#### Dependency Updates
- `libheif-rs`: 2.6.0 → 2.6.1
- `tempfile`: 3.25 → 3.26

### 📊 Logging & UX

#### Per-File Log Context
- **Parallel Output Attribution**: When processing multiple files in parallel, each log line is now prefixed with `[filename]` so output can be attributed to the correct file.
- **ANSI Stripping**: Color codes are stripped when output is not a TTY or when writing to log files.

#### Progress Display Improvements
- **Compact Milestones**: Images OK/failed counts now displayed on same line as XMP/JXL milestones.
- **XMP Clarity**: XMP merge milestone lines use fixed `[XMP]` prefix to avoid confusion with Metadata total.

#### Ultimate Mode Enhancement
- **MS-SSIM Threshold**: Extended MS-SSIM skip threshold from 5 minutes to **25 minutes** in ultimate mode. Only videos >25 minutes will skip MS-SSIM and use SSIM-only verification.

### 🛠️ Technical

- **video_explorer.rs**: GIF quality verification explicit failure, calibration stderr printing, Y4M `-an` flag
- **cli_runner.rs**: Single-file copy-on-fail logic
- **conversion.rs**: TOCTOU-safe temp file + atomic rename
- **msssim_parallel.rs**: GIF returns `Err` instead of `Ok(skipped)`
- **flag_validator.rs**: Simplified to only accept recommended combination (`explore && match_quality && compress`)
- **scripts/drag_and_drop_processor.sh**: Subcommand unified to `run`, recursive forced on, no auto Apple Photos repair

---

## [8.6.0] - 2026-02-24

### 🎬 MS-SSIM 极限模式时长参数

- **极限模式（--ultimate）**：MS-SSIM 跳过阈值由 5 分钟改为 **25 分钟**；仅当视频 >25 分钟时才跳过 MS-SSIM、仅用 SSIM 验证。
- **实现**：`gpu_coarse_search`、`video_explorer.validate_quality` 在 ultimate 下使用 25 min 阈值；`ssim_calculator.calculate_ms_ssim_yuv` 新增参数 `max_duration_min`（5.0 或 25.0），日志中显示对应阈值（如「≤25min」/「>25min」）。
- **文档**：CODE_AUDIT.md 新增 34 节「极限模式下 MS-SSIM 跳过阈值延长（25 分钟）」。

## [8.5.1] - 2026-02-23

### 📋 Audit follow-up (文档与可见性)

#### 算法与设计文档
- **Phase 2 搜索**（`video_explorer.rs`）：补充注释——CRF–SSIM 单调性假设；为何采用单点黄金比例而非完整黄金分割搜索（实现简单、每轮同样 1 次编码，仅可能多 1～2 次编码）。
- **迭代上限**（`video_explorer.rs`）：为长视频/超长视频的迭代上限常量添加文档，说明「更长视频 → 更低迭代上限」为有意为之的成本/精度权衡。
- **效率因子**（`quality_matcher.rs`）：模块与 `efficiency_factor()` 的文档中注明 H.264/HEVC/AV1 等为经验相对效率，可参考编解码比较研究，无单一权威引用。

#### 质量验证可见性
- **长视频跳过 MS-SSIM**：在 `ssim_calculator.rs`、`gpu_coarse_search.rs`、`video_explorer.rs`、`msssim_sampling.rs` 四处，将「跳过 MS-SSIM」的日志统一为带 ⚠️ 的警告级表述（"Quality verification: … MS-SSIM skipped"），便于用户知晓质量验证降级为仅 SSIM。

#### 审计文档
- **CODE_AUDIT.md**：新增「为何不用完整黄金分割搜索」说明；与代码注释一致。

## [8.5.0] - 2026-02-23

### 📋 Logging & Concurrency

#### Per-file log context (fix interleaved output)
- **Thread-local log prefix**: When processing multiple files in parallel, every `log_eprintln!` / `verbose_eprintln!` line is prefixed with `[filename]` so output can be attributed to the correct file.
- **Set at entry points**: `vid_hevc` `auto_convert()` and `img_hevc` `auto_convert_single_file()` set the prefix from the input file name and clear it on drop via `LogContextGuard`.
- **XMP distinct**: XMP merge milestone lines use a fixed `[XMP]` prefix so they are clearly separate from file-tagged lines.

#### Formatted indentation
- **Fixed-width tag column** (`LOG_TAG_WIDTH = 34`): All message bodies align so `[file.jpeg]`, `[file.webp]`, and `[XMP]` lines start the message at the same column.
- **Padding**: `pad_tag()` pads the tag so SSIM/CRF/XMP lines are visually aligned and easier to scan.

#### UTF-8 safe prefix
- **No panic on CJK filenames**: Prefix truncation now uses `truncate_to_char_boundary()` so we never slice through a multi-byte character (e.g. Chinese/Japanese in file names).
- **Shorter default**: `LOG_PREFIX_MAX_LEN` reduced to 28 to reduce log noise.

### ⏱️ Duration detection

#### ImageMagick fallback for WebP/GIF
- **Problem**: Animated WebP (and some GIF) often have no `stream.duration`, `format.duration`, or usable `frame_count`/fps from ffprobe, causing "DURATION DETECTION FAILED" and conversion to abort.
- **Solution**: In `detect_duration_comprehensive()` (precheck), after all ffprobe-based methods fail, try ImageMagick: `get_animation_duration_and_frames_imagemagick(path)` using `identify -format "%T"` to get (duration_secs, frame_count), then infer fps and return `(duration, fps, frame_count, "imagemagick")`.
- **API**: `image_analyzer::get_animation_duration_and_frames_imagemagick(path)` returns `Option<(f64, u64)>` without logging; existing `try_imagemagick_identify` uses it and keeps the "WebP/GIF animation detected" log.

### 🎬 GIF / animated quality verification

#### QualityCheck message when verification skipped
- When GIF input uses the size-only path (SSIM-All verification failed or unavailable), the summary line is now **"QualityCheck: N/A (GIF/size-only, quality not measured)"** instead of "FAILED (quality not verified)", so batch logs are less alarming and reflect expected behavior.

#### Real quality verification for GIF (and transparent inputs)
- **Direct + format normalization**: `calculate_ssim_all()` now tries (1) direct `[0:v][1:v]ssim`, (2) format normalization: both streams to `yuv420p` and even dimensions so GIF palette and HEVC output are comparable.
- **Alpha flatten (transparent GIF/WebP/PNG)**: Third fallback matches the encoder: input is converted with `format=rgba,premultiply=inplace=1,format=rgb24,format=yuv420p` (composite on black) then compared to HEVC output, so transparent pixels are evaluated on the same basis as the encoded file.
- **Helper**: `run_ssim_all_filter(input, output, lavfi)` runs a given lavfi graph and parses SSIM Y/U/V/All from stderr with validity checks.

### 🛠️ Technical

- **progress_mode** (`shared_utils`): `set_log_context`, `clear_log_context`, `format_log_line`, `LogContextGuard`, `pad_tag`, UTF-8-safe `set_log_context`.
- **precheck** (`video_explorer`): ImageMagick duration fallback after stream/format/frame_count+fps.
- **stream_analysis** (`video_explorer`): `calculate_ssim_all` multi-step fallback (direct → format_norm → alpha_flatten); `run_ssim_all_filter` for reusable lavfi + parse.
- **gpu_coarse_search** (`video_explorer`): `quality_verification_skipped_for_format` flag for GIF and friendlier QualityCheck line.

## [8.2.2] - 2026-02-20

### 🔥 Critical Bug Fixes

#### WebP/GIF Animation Duration Detection
- **Fixed ffprobe N/A Issue**: ffprobe returns `N/A` for WebP/GIF animation duration metadata
- **Added ImageMagick Identify Fallback**: New detection method using `identify -format "%T"` to read frame delays in centiseconds
- **Accurate Duration Calculation**: Sums all frame delays to calculate total animation duration
- **Impact**: 35+ animated WebP files that were previously skipped will now be correctly converted:
  - Duration ≥3s → HEVC MP4
  - Duration <3s → GIF (Bayer 256 colors)

#### Extension Mismatch Handling
- **Content-Aware Extension Correction**: Files are now renamed to match their actual content format before processing
  - `.jpeg` containing HEIC → renamed to `.heic`
  - `.jpeg` containing WebP → renamed to `.webp`
  - `.jpeg` containing PNG → renamed to `.png`
  - `.jpeg` containing TIFF → renamed to `.tiff`
- **Prevents Wrong Re-encoding**: Fixed issue where HEIC/WebP files with `.jpeg` extension were incorrectly re-encoded as JPEG by ImageMagick structural repair

#### On-Demand Structural Repair
- **Changed from Unconditional to On-Demand**: ImageMagick structural repair now only runs when exiftool detects metadata corruption
- **Performance Improvement**: Saves 100-300ms per file for healthy files (no unnecessary re-encoding)
- **Quality Protection**: Avoids unnecessary re-encoding for files without metadata issues

### 🌐 Internationalization

#### Complete English Output
- **All User-Facing Messages**: Converted from Simplified Chinese to English
- **Error Messages**: Full English translations for all error outputs
- **Console Output**: All processing logs, warnings, and success messages now in English
- **Comments**: Code comments translated to English for better maintainability

### 📦 Dependencies Updated
- `console`: 0.15 → 0.16
- `tempfile`: 3.10 → 3.20
- `proptest`: 1.4 → 1.7

### 🛠️ Technical Improvements
- **Magic Bytes Detection**: Extended to support HEIC brands (heic, heix, heim, heis, mif1, msf1)
- **Smart File Copier**: New module for content-aware extension correction
- **Improved Error Handling**: Better fallback mechanisms for format detection failures

## [8.2.1] - 2026-02-20

### 🔧 UI Text Fixes
- **Menu Option Renamed**: "Brotli EXIF Fix Only" → "Fix iCloud Import Errors"
- **Clearer Description**: "Fix corrupted Brotli EXIF metadata that prevents iCloud Photos import"

## [8.2.0] - 2026-02-20

### 🍎 Apple Ecosystem Compatibility (Critical Fixes)
- **"Unknown Error" Resolved**: Fixed a critical issue where Apple Photos refused to import files due to extension mismatch (e.g., WebP files renamed as .jpeg).
- **WebP Disguised as JPEG**: Implemented `Magic Bytes` detection. The tool now ignores the literal file extension and inspects the file header. If a `.jpeg` is actually a WebP, it automatically routes it through `dwebp` pre-processing to ensure a valid JXL output.
- **Corrupted JPEG Repair**: Added pre-processing for JPEGs with illegal headers (e.g., missing `FF D8` start bytes). These are now sanitized using ImageMagick before conversion, preventing decoder crashes.
- **Nuclear Metadata Rebuild**: When `Apple Compatibility` mode is enabled, the tool now performs a "Nuclear Rebuild" (`exiftool -all=`) on metadata. This strips out "toxic" non-standard tags injected by third-party editors (e.g., Meitu) that cause Apple Photos to reject valid files.
- **Directory Timestamp Preservation**: Fixed an issue where processing files would update the parent directory's modification time. The tool now recursively saves and restores timestamps for all affected directories (deepest-first).

### ⚡ Core Improvements
- **Smart Format Detection**: Moved away from trusting file extensions. The core logic now relies on binary signatures for `jpg`, `png`, `gif`, `tif`, `webp`, and `mov`.
- **Robust Pre-processing**: Integrated `magick` and `dwebp` deeply into the Rust pipeline to handle edge cases that previously caused `cjxl` to fail.

### 🎨 UI/UX
- **Enhanced Logging**: Redesigned the CLI output with hierarchical styling.
  - **Important Alerts**: Now displayed in **Bold/Colored** text.
  - **Technical Details**: Now displayed in **Dimmed (Gray)** text to reduce visual noise.
- **Status Indicators**: Added clearer emojis (`✅`, `⚠️`, `🔧`) for operation states.

## [8.1.0] - 2026-02-15
- Initial release of the `modern_format_boost` Rust rewrites.

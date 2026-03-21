# Changelog

All notable changes to this project will be documented in this file.

**Version scheme:** As of this release, the project uses **0.8.x** versioning (replacing the previous 8.x scheme).

## [0.10.87] - 2026-03-22

### Fixed
- 🎞️ **Animated quality metrics no longer crash on odd/even dimension mismatches**: `VMAF-Y`, `PSNR-UV`, and `MS-SSIM` now normalize both reference and encoded streams to the same shared even resolution before running ffmpeg/libvmaf filters. This fixes `Error reinitializing filters` / `Invalid argument (-22)` failures seen during GIF and other animated-image CRF search when one side landed on odd dimensions.

### 🛡️ Comprehensive Privacy Purge & Repository Hardening
- **Repository-Wide History Sanitization**: Executed deep Git history rewrite to completely eliminate accidental metadata, test assets, and sensitive path leaks from the global revision graph.
- **Historical Documentation Archival**: Successfully extracted and localized 140+ legacy technical documents (Algorithms, Audits, Manuals) to the local `logs/` directory, while removing them from the remote Git footprint to ensure a lean, production-focused codebase.
- **Dependency Architecture Bifurcation**:
    - **Main (Stable)**: Locked to high-stability `crates.io` dependencies (e.g., `image v0.25.5`) for maximum reliability.
    - **Nightly (Edge)**: Synchronized with the absolute latest upstream iterations from GitHub Git sources (e.g., `image v0.25.x HEAD`) to support rapid iteration.
- **Changelog Reconstruction**: Recovered 2200+ lines of archival history following repository restructuring.

## [0.10.86] - 2026-03-21

### Changed
- 🔢 **Release finalized**: unified versioning bumped to 0.10.86 to seal the v0.10.85 feature set and documentation.
- 🔢 **Version references are unified again**: workspace version bumped to 0.10.86, and cache-version examples/docs now match the real mapping (0.10.86 -> 1086).

## [0.10.85] - 2026-03-20

### 🚀 Key Improvements since v0.10.82

#### 🖥️ Runtime & GUI Hardening
- **Bootstrapped Environments**: Added robust environment stabilization (PATH, Cargo, Locale) for GUI and Finder-launched sessions, eliminating silent failures in sparse terminal environments.
- **Terminal-Aware Progress**: CoarseProgressBar now dynamically adapts to terminal width, preventing redraw artifacts and line-wrapping in narrow CLI windows.
- **Atomic Renaming**: Optimized output commitment on Windows to use direct atomic renaming (`MoveFileExW`), ensuring data integrity during process interruptions.

#### 💾 Reliability & Storage Management
- **Disk Exhaustion Pausing**: All batch tools now detect storage exhaustion mid-run, automatically pausing work, releasing locks, and preserving progress for easy resumption.
- **Signature-Bound Checkpoints**: Resume state is now validated against file signatures (size/mtime/mtime/btime) and cache versions, preventing stale or inconsistent resume attempts.
- **Automatic Resume Reset**: Manually deleting an output folder now automatically triggers a full-run reset, eliminating the need to manually clear checkpoint files.

#### 🎞️ Video Encoding & Quality
- **CRF Warm-Start Hints**: Refined the video CRF search anchor. Cached results now act as intelligent hints rather than rigid overrides, allowing for better adaptation to current system conditions.
- **Best-Effort Persistence**: "Quality Miss" scenarios now store their results as reusable CRF hints, optimizing the next attempt even if the initial target wasn't met.
- **Stream Mapping Fix**: Resolved odd-height cover art encoding failures by locking libx265 re-encoding to the primary video stream only.

#### 📢 Error Visibility & Recovery
- **Loud Failures (The "Wake Up All Silent Errors" Update)**: Surfaced dozens of previously silent failure points, including background thread panics, GPU watchdog issues, metadata preservation errors, and cache write conflicts.
- **Probing Portability**: Standardized PID age detection across macOS and Linux, reducing false "stale lock" warnings while maintaining strict concurrency safety.

#### 📦 Maintenance & Infrastructure
- **Dependency Refresh**: Synchronized all workspace dependencies to their latest compatible versions across crates.io and GitHub sources.
- **Metadata Scoping**: Restored precise scoping for Finder branding, ensuring MFB badges are only applied to successfully converted output files.
- **Legacy Cleanup**: Removed redundant release notes and stale documentation from the repository root.

## [0.10.83] - 2026-03-19

### Fixed
- 🏷️ **Finder comment branding is now scoped to conversion output only**: `append_mfb_branding` was previously called inside `preserve_pro`, which fires on every metadata-preservation operation (including non-conversion paths). It is now called exclusively inside `commit_temp_to_output_with_metadata` after a successful atomic rename, ensuring the Finder comment is only written to files that were actually converted by MFB.
- 🗑️ **Original-file deletion failures are no longer silent**: `safe_delete_original` errors in `finalize_conversion` are now propagated instead of being discarded with `let _ =`, so a failed delete surfaces as a conversion error rather than being silently ignored.

## [0.10.82] - 2026-03-18

### Fixed
- 📽️ **FFmpeg Stream Mapping**: Added explicit mapping `-map 0:v:0 -map 0:a? -map 0:s?` to the video encoding pipeline to ensure only the primary video stream is re-encoded, fixing odd-height cover art errors.
- 🛡️ **Atomic Output Switch**: Optimized `commit_temp_to_output_with_metadata` with direct atomic renaming (`MoveFileExW` on Windows) to prevent data loss during interruptions.
- 🔒 **Path and Process Hardening**: Hardened output path generation (rejecting control characters/symlinks) and standardized Unix checkpoint lock age detection using `ps -o etimes`.
- 📋 **Universal Loud Failures**: This milestone represents a project-wide push to surface previously "silent failures" into explicit, actionable errors:
    - **Recovery & Batch Traversal**: Explicit warnings for fallback copies, run-log setup, and `walkdir` traversal failures.
    - **PNG & Image Analysis**: Stricter corruption checking for PNG chunks and observable fallback explanations for JPEG/JXL duration probes.
    - **Metadata Preservation**: Native `xattr`/ACL/permission/timestamp preservation on macOS/Linux/Windows now warns on real failures.
    - **Resource & Cache**: Warns on RAM/disk/ffprobe-parse failures; Surfaces SQLite schema migration and POST-write cache size enforcement errors.
    - **Cleanup & Rollback**: Temp-output guards and video quality cleanup failures are now fully visible instead of silently leaving stale artifacts.
- ⏸️ **Mid-run disk exhaustion now pauses instead of cascading failures**: All four batch tools now cleanly pause, release locks, and preserve progress when storage runs out.

## [0.10.81] - 2026-03-17

### 🚀 Key Highlights (Since v0.10.78)

#### 🔄 Centralized Progress & Batch Resume (v0.10.79+)
- 🌍 **Zero Directory Pollution**: All processing metadata folders (`.mfb_progress`) have been consolidated into a single, hidden location in the user's home directory (`~/.mfb_progress/`). Improved Privacy: Keeps your photo and video directories completely clean throughout the processing lifecycle.
- 🛡️ **Atomic Resume Framework**: Introduced a robust, thread-safe checkpoint system. Simply restarting an interrupted job will skip already completed files with millisecond-level detection.
- **Canonical Path Hashing**: Progress is keyed by the absolute canonical path hash of the target directory, ensuring reliable tracking even across symbolic links.
- 🗑️ **Automatic Lifecycle Management**: Progress data for a specific folder is automatically and securely purged upon a 100% successful completion.

#### 🔠 Extension Standardization
- 🔠 **Uppercase File Extensions**: Standardized all output extensions to uppercase across all tools (e.g., `.JXL`, `.MP4`, `.MKV`, `.AVIF`, `.WEBM`) for better visibility in professional file managers and macOS Finder.
- 🎯 **Path Logic Refinement**: Updated the internal `determine_output_path` API to enforce uppercase extensions while accurately preserving filename stems.

#### 🛡️ System Robustness & UI Improvements
- 🛡️ **Shell Path Escaping (macOS App)**: Fixed a critical bug in the macOS App wrapper's path quoting logic, correctly handling single quotes, emojis, and shell metacharacters.
- 🧹 **Data Purge Branding**: Renamed "Clean Cache" to "Purge Processing Data" across all maintenance scripts (drag_and_drop_processor.sh, cache_cleaner.sh).
- ⚖️ **Thread-Safe Testing**: Refactored the internal `CheckpointManager` test suite to use isolated temporary directories, avoiding CI/CD test collisions.

## [0.10.80] - 2026-03-16

### Added
- 🌍 **Centralized Progress Tracking**: Moved all `.mfb_progress` folders to `~/.mfb_progress/`.
- 🛡️ **Enhanced UI Warnings**: Added prominent backup warnings to the drag-and-drop terminal interface.

### Changed
- 🧹 **Data Purge Branding**: Renamed "Clean Cache" to "Purge Processing Data".
- 🛠️ **Robust Cleanup**: Updated `cache_cleaner.sh` to include centralized progress data in the purging process.
- 🔒 **Thread-Safe Test Suite**: Refactored `CheckpointManager` unit tests for reliable multi-threaded execution.

## [0.10.78] - 2026-03-15

### 🏆 Documentation & Transparency
- 📖 **Complete README Overhaul**: Rewritten with a professional bilingual (English/Chinese) structure and deep technical pipeline explanations.
- ⚠️ **Stability Disclaimer**: Added guidance highlighting HEVC maturity lead over AV1 variants for production tasks.
- ⚖️ **License Finalization**: Restored full runtime dependency license tables for compliance.

### 🛡️ Metadata & Data Integrity (Massive Overhaul)
- 🗂️ **Multi-Platform Preservation**:
    - **macOS**: Added native Date Added (`kMDItemDateAdded`) and Finder Tag preservation via `copyfile` and `setattrlist`.
    - **Windows**: Added Alternate Data Streams (ADS) support via PowerShell.
    - **Linux**: Standardized ACL restoration using `setfacl --restore`.
- 📅 **QuickTime/EXIF Sync**: Overhauled `fix_quicktime_dates` to synchronize all capture date fields forcefully.
- 🎨 **ICC Profiles**: Fixed ICC color space loss in JXL conversion; all JXL outputs now manually inject and verify source ICC profiles.
- 💾 **Disk Space Pre-Check**: All tools now perform a pre-batch disk space validation.

### 🎬 Video Processing Stability
- 🔧 **Odd-Dimension Fix**: Resolved EINVAL (-22) errors by adding automatic `scale=trunc(iw/2)*2` normalization.
- 🛡️ **Ctrl+C Guard**: Unified the 4.5-minute confirmation guard across all binaries.

### 🧪 Algorithmic Improvements
- 🎯 **PNG Quantization Detection (Meme Score v3)**: Added RGB-weighted banding analysis and dithering recognition for improved icons/pixel-art accuracy.
- ✨ **AV1 Tools Parity**: Brought `img-av1` and `vid-av1` up to feature parity with HEVC tools, including unified finalization checks.

## [0.10.45] - 2026-03-14

## [0.10.66] - 2026-03-12

### Fixed
- 🔓 **HEIC Security Limits (Critical - Complete Fix)**: Correctly implemented security limits with proper API usage.
- **Root Cause Analysis**: Fixed issues where v0.10.65 used non-existent APIs and failed to propagate the `v1_21` feature flag.
- **Solution**: Implemented the correct three-step API (`HeifContext::new()` → `set_security_limits()` → `read_bytes()`).
- **Feature Propagation**: Added `v1_21` to default features in `shared_utils`, `img_hevc`, and `img_av1`.
- **Limits Increased**: Memory 15GB (was 7GB), `ipco` boxes 50,000 (was 10,000).
- **Fallback Strategy**: Restored complete 3-layer fallback (main → `ftyp` scan → file read). Fixes "Maximum number of child boxes (100) in 'ipco' box exceeded" errors.
- 🔧 **Code Quality**: Fixed all clippy warnings in library code (simplified logic, fixed lazy evaluations).

### Technical Details
- **Correct API Usage**:
  ```rust
  let mut ctx = HeifContext::new()?;           // 1. Create empty context
  ctx.set_security_limits(&limits)?;           // 2. Set limits BEFORE reading
  ctx.read_bytes(&data)?;                      // 3. Read with limits applied
  ```
- **Security Limits**: `max_total_memory`: 15GB, `max_children_per_box`: 50,000, `max_items`: 500,000, `max_components`: 50,000.

### Added
- `verify_heic_config.sh`: Verification script to check all HEIC security configurations.
- `HEIC_SECURITY_CONFIG.md`: Comprehensive documentation of HEIC security configuration.

## [0.10.64] - 2026-03-11

### Highlights (v0.10.9 → v0.10.64)
- 🔒 **Security & Privacy**: Permanently removed AI tool configs from Git history (1,724 commits cleaned; repo size reduced to 78MB).
- 🔓 **HEIC Processing**: Increased security limits (6GB memory, 10k `ipco` children). Fixed lossless detection cache bug for `RExt` profile + 4:4:4 chroma.
- 🎯 **Quality & Detection**: 3D Quality Gate (VMAF-Y ≥93.0, PSNR-UV ≥35.0, CAMBI ≤5.0). 0.01-precision CRF fine-tuning with sprint & backtrack optimization.
- 🚀 **Performance**: Global CRF cache with warm start; Cache version binding for auto-invalidation; JPEG fast path header analysis.
- 📦 **Dependencies**: Branch strategy finalized (Main stable vs Nightly GitHub sources).
- 🎨 **UI & Logging**: 24-bit TrueColor UI with video milestones (V:, X:, P:, I:). Unified error system with classification.
- 🔧 **Technical**: Unique 8-char UUID temp files; Apple ecosystem support (AAE sidecars, iPhone VFR, iCloud metadata).

## [0.10.9] - 2025-12-28

### Release v0.10.9
- **Robust ImageMagick Fallback Pipeline**: Fixed a critical logic bug where the ImageMagick fallback was never called for `img-hevc` and `img-av1` after direct `cjxl` failure.
- **Enhanced Grayscale ICC Conflict Handling**: Optimized recovery for grayscale images with incompatible RGB ICC profiles (stripping profiles and attempting 16-bit/8-bit conversion).
- **Fixed Silent Failures**: Added detailed logging for multi-attempt fallbacks (✅/❌ status icons).
- **Broader Error Detection**: Expanded `is_decode_or_pixel_cjxl_error` to catch more pixel data and decoding errors.
- **Code Quality**: Resolved unused variable compiler warnings.




### Mega-Release: Cumulative Evolution (v0.10.9 → v0.10.45)

#### High-Fidelity Algorithm & Quality Logic
- **Extreme Mode Saturation Search**: Implemented **0.01-precision** CRF fine-tuning to ensure video quality reaches the "Physical Red Line" (Saturation).
- **3D 3rd-Generation Quality Gate**: Integrated **VMAF-Y** (Perceptual), **PSNR-UV** (Chroma保真度), and **CAMBI** (Banding detection) for exhaustive verification.
- **Sprint & Backtrack Optimization**: Search performance leap using double-step sprints (up to 1.6x) and precise 0.1-step rollbacks on overshoot.
- **Unified 1MB Size Tolerance**: Standardized size increase checks (1,048,576 bytes) workspace-wide to ensure high-quality leaps remain balanced with file size.

#### Image Processing Intelligence (v2)
- **JPEG Lossless Transcoding**: Mathematical bit-exact reconstruction using direct DQT mapping into **JXL varDCT** profiles.
- **Heuristic v2 Estimation Engine**: Revolutionary quality detection using Efficiency-Weighted BPP and **Image Entropy (Edge Density/Complexity)** estimation.
- **Lossless Detection Parity**: Deterministic identification for Modular JXL, WebP-L, and High-Bit-Depth (10-bit+) sources.
- **Meme Score v3**: High-frame-rate aware heuristic engine for smart decisions on modern animations and Live2D stickers.
- **Consistent High-Fidelity Path**: Unified all legacy static sources to the `Quality 100` (`d=0.1`) route unless lossless is recommended.

#### Professional UI & Logging Infrastructure
- **24-bit TrueColor Terminal Support**: Implemented a sophisticated, brand-aligned TrueColor UI with semantic "Card"-style summaries.
- **Minimalist Video Milestones**: Introduced abbreviated trackers (`V:`, `X:`, `P:`, `I:`) specifically tailored for high-concurrency video processing logs.
- **Terminal Title-Bar Spinner**: Isolated background progress indicators using OSC escape sequences, preventing content clutter and TTY interference.
- **Unified Error Classification**: Consolidated all project failures into a central system: 🚨 Critical, ⚠️ Rare, 📋 Metadata, and 🔧 Pipeline errors.

#### Ecosystem & Safety Enhancements
- **Apple Ecosystem Parity**: Full support for **AAE sidecars**, iPhone VFR (Slow-Mo) detection, and iCloud-standard metadata preservation.
- **Collision-Resistant Temp Files**: Introduced 8-character random UUID prefixes for all temporary assets to ensure thread-safe processing and reliable cleanup.
- **Ctrl+C (SIGINT) Job Guard**: Resilient interruption protection using libc-poll events, job duration awareness (4.5m), and auto-resume logic.

## [0.10.44] - 2026-03-14

### Fixed
- **Hardcoded Quality Degradation in Image Routing**:
  - **Unified Quality 100 Path**: Eliminated hardcoded `d=1.0` routing for palette-quantized PNG and GIF sources.
  - **Static GIF Routing Unification**: 1-frame GIFs now correctly follow the `pixel_analysis` decision path, enabling `d=0.0` (Lossless) when appropriate.
  - **Startup Log Alignment**: Updated the initialization banner to correctly reflect the new `d=0.0/0.1` distance standards for static images.
  - **Doc-Comment Correction**: Updated developer documentation to reflect the current high-fidelity distance standards.

## [0.10.43] - 2026-03-14

### Added
- **Minimalist Abbreviated Milestones for Video Mode**:
  - Implemented `IS_VIDEO_MODE` detection and minimalist milestone formatting specifically for video tools.
  - Shortened all milestone labels to single characters (`X`, `I`, `P`, `V`) for maximum terminal space efficiency.
  - **Video-Specific Tracking**: `vid_hevc` and `vid_av1` now track and display video milestones (`V:`) and preprocessing (`P:`) instead of image counters.
  - **Dynamic XMP Shorthand**: Added `X:` (XMP) support to video mode, automatically appearing only when sidecar merges occur.
  - **Refined Aesthetics**: Removed the 📊 chart icon and extra spacing in video mode for a cleaner, stage-focused log appearance.

### Fixed
- **Format String Errors**: Resolved critical `format!` macro argument count mismatches in the milestone reporting logic.
- **Redundant Logic**: Cleaned up duplicate `enable_quiet_mode` definitions in `shared_utils`.
- **Milestone Hook Integration**: Fixed missing video success/failure hooks in the shared CLI runner, ensuring accurate progress tracking for all video tools.

## [0.10.42] - 2026-03-13

### Changed
- **Unified Milestone Statistics**: Milestone statistics (XMP, Img, Pre) are now appended to *every* image processing log line, including multi-line fallback and diagnostic messages.
  - **Multi-line Support**: Diagnostic messages such as `[QUALITY FALLBACK]` and `[Smart Fix]` now display milestones on every line for perfect terminal alignment.
  - **Consistent Progress Tracker**: The statistics bar (`│ 📊 XMP: ... Img: ... Pre: ...`) is now visible from the very first log entry, ensuring the conversion status is always available.
  - **Full Log Audit**: All tracing and verbose logs in the run log file now also include milestones, providing a synchronized timeline of system state and progress.
- **Improved Alignment Logic**: Re-engineered the padding and ANSI-stripping logic to ensure statistics are perfectly aligned at column 65 across all log levels.

## [0.10.41] - 2026-03-13

### Changed
- **Terminal Noise Reduction**: JPEG-related conversion logs (e.g., JPEG to JXL lossless transcoding) are now hidden from the terminal by default.
  - **Quiet Success**: These operations are considered routine and low-risk; hiding them keeps the terminal focused on more significant conversions (HEVC, AV1).
  - **Full Accountability**: All JPEG conversion details remain fully recorded in the run log file for auditing and verification.
  - **Opt-in Visibility**: Use the `--verbose` flag to restore these logs to the terminal if needed.

## [0.10.40] - 2026-03-13

### Added
- **JSON-based Image Classification Engine**: Refactored the hardcoded classification logic into a flexible, data-driven rule engine.
  - **Extensible Rules**: New categories added: `MOBILE_SCREENSHOT`, `GAME_CAPTURE`, `WEB_UI`, `MAP`, `DOCUMENT`, `NIGHT_PHOTO`, `MACRO_PHOTO`, and `MEME`.
  - **Dynamic Configuration**: Classification logic is now driven by `image_classifiers.json` (embedded in binary), allowing for rapid updates to thresholds, quality adjustments, and format recommendations.
  - **Advanced Matching**: Rules now support multi-dimensional matching across complexity, edge density, color diversity, texture variance, noise, sharpness, contrast, aspect ratio, and resolution.
- **Improved Metadata Logic**: Transitioned `ImageContentType` to a rich data structure that carries its own encoding bias and recommended formats directly from the rule engine.

## [0.10.39] - 2026-03-13

### Added
- **Image Quality Metrics in Logs**: Added pixel-based quality analysis to terminal output.
  - **Dynamic Labels**: Automated detection of content types (`PHOTO`, `SCREENSHOT`, `ARTWORK`, etc.) and quality factors (e.g., `Q=95 Excellence`).
  - **Improved Formatting**: Success logs now prominently display quality metrics using a clean `✅ TYPE | QUALITY | ACTION` format.
  - **Log Realignment**: Re-calculated padding to ensure statistics (XMP, Img, Pre) remain perfectly aligned at the terminal's right margin.
- **Enhanced Image Analysis**: Integrated `ImageAnalysis` with a new `quality_summary` engine for consistent reporting across HEVC and AV1 tools.

### Added
- **Container Overhead Tolerance**: Added 1MB tolerance for container overhead in `vid_hevc` size checks. Total file size is now accepted if it exceeds original size by less than 1MB, provided the video stream itself was compressed.
- **Duplicate Path Diagnostics**: Enhanced "Already exists" logging in `smart_file_copier` to show file size and accessibility status, aiding in troubleshooting.

### Fixed
- **Temp File Deletion**: Fixed an issue where temporary files (`.gpu_temp.mov`) were left behind when GPU coarse search failed or was interrupted.
- **PSNR Calculation**: Fixed "PSNR calc failed" errors in GPU acceleration module by using explicit filter graph syntax `[0:v][1:v]psnr` instead of implicit inputs.

## [0.10.37] - 2026-03-13

### Added
- **Security Enhancement: Unique Temp Files**: Added 8-character random string (e.g., `.tmp.A7b2K9xZ.mp4`) to temporary file names in `shared_utils`. This prevents collisions with user-named files, enables safe concurrent processing, and ensures that cleanup operations only target program-generated files.
- **Sampling Duration Increase**: Increased GPU and CPU sampling duration in Ultimate Mode by 15.0s each.
  - GPU: 45.0s → 60.0s (segmented: 50.0s → 65.0s)
  - CPU (Calibration): 15.0s → 30.0s
- **Phase 4 Metrics Display**: Added VMAF and PSNR (UV) display to Phase 4 (0.01-granularity fine-tune) logs for consistency with CPU/GPU phases.
- **Diagnostic Logging**: Added temp file verification and detailed error messages before commit to diagnose "No such file or directory" errors.

### Fixed
- **Temp File Cleanup**: Temp files (`.tmp.` pattern) are now properly cleaned up when processing fails or is interrupted, preventing leftover files in output directory.
- **Dependency Update**: Updated all workspace dependencies to their latest compatible versions.

## [0.10.36] - 2026-03-13

### Added
- **Unified Error Handling System**: Consolidated 6 error handling modules into `unified_error.rs`
  - Centralized error types (VidQualityError, ImgQualityError, AppError) into `UnifiedError`
  - Added comprehensive error classification (Fatal/Recoverable/Optional)
  - Implemented user-friendly messages with emoji indicators
  - Provided convenient constructors and context methods
- **Modern 24-bit True Color Logging System**: New logging infrastructure
  - Added `enhanced_logging.rs` with full log level hierarchy (ERROR > WARN > INFO > DEBUG > TRACE)
  - Added `terminal_logging.rs` with color-safe output mechanism
  - Support for 24-bit true color terminal output
  - Added upstream tool logger (prevents silencing upstream logs)
  - Unified visual style across all logging paths

### Changed
- **Restored Sprint & Backtrack Mechanism**: Re-enabled accelerated search in Phase 3
  - **Sprint**: Double step (0.1 → 0.2 → 0.4...max 1.6) on consecutive successes
  - **Backtrack**: Reset to 0.1 precision on overshoot for accuracy
- **Enhanced Quality Verification**: Improved error handling for missing VMAF/PSNR metrics
- **Improved Log Formatting**: Better GPU/CPU phase distinction, cleaner fallback messages
- **Code Quality**: Removed silent fallback values and dead modules

### Fixed
- **Phase 2 Duplicate Output**: Fixed duplicate logging in Phase 2 when ultimate_mode is enabled
  - Moved quality metrics check to only run when compression fails
  - Each CRF now outputs only once during exploration
- **Phase 2 Early Termination**: Fixed Phase 2 continuing after finding compression point
  - Now correctly stops immediately after finding first compressible CRF
  - Properly transitions to Phase 3 without wasted iterations
- **Phase 3 False Quality Collapse Detection**: Fixed incorrect "quality collapse" detection
  - Now distinguishes between size wall (file too large) and actual quality degradation
  - Only triggers failure credibility when quality metrics truly fail thresholds
  - Size wall without quality issues no longer stops exploration prematurely
- **PSNR-UV Threshold Consistency**: Unified PSNR_UV_MIN threshold across all phases
  - Changed from 38.0 dB to 35.0 dB (4 locations)
  - More realistic threshold matching actual video quality characteristics
  - Prevents false quality gate failures for high-VMAF content
- **x265 Encoder Logging Verbosity**: Reduced terminal noise during exploration
  - Changed info-level logs to debug-level in encode_with_x265, encode_to_hevc, encode_y4m_direct, mux_hevc_to_container
  - Exploration phase now runs silently, details available in debug mode
  - Aligns with plan.json T04-8: "Terminal output should show only key summary information"
- **Quality Verification Log Clarity**: Improved PSNR-UV pass/fail reporting
  - Now shows individual U and V channel results: `U=38.38 dB ✅, V=35.67 dB ✅`
  - Clear indication of which channel passes/fails threshold
  - Easier to diagnose quality issues at a glance
- **Early Insight Log Transparency**: Added quality metrics display when early insight triggers
  - Shows VMAF-Y and PSNR-UV values when quality plateau is detected
  - Helps users understand why exploration stopped early
  - Provides visibility into quality gate decisions
- **GPU Utilization in Ultimate Mode**: Increased GPU exploration precision and iterations
  - GPU initial step: 2.0 → 0.5 in ultimate mode (4x more precise)
  - GPU minimum step: 0.5 → 0.1 in ultimate mode (5x more precise)
  - GPU decay factor: 0.5 → 0.6 in ultimate mode (slower convergence = more iterations)
  - GPU max wall hits: 4 → 6 in ultimate mode (50% more attempts)
  - GPU Stage 1 threshold: 4.0 → 2.0 in ultimate mode (triggers more often)
  - GPU sample duration: 90s → 45s in ultimate mode (prevent timeout)
  - GPU segment duration: 25s → 10s in ultimate mode (5 segments = 50s total)
  - GPU skip threshold: 500KB → 100KB in ultimate mode
  - GPU skip duration: 3.0s → 1.0s in ultimate mode
  - **GPU search logs now visible in ultimate mode** (was silent, causing confusion)
  - More GPU iterations with shorter samples = higher utilization without timeout
- **PSNR Calculation Reliability**: Improved PSNR calculation with better error handling
  - Added stats_file output for more reliable parsing
  - Multiple parsing strategies (psnr_avg, average)
  - Detailed error messages when parsing fails
  - Prevents "PSNR calc failed, fallback to size-only" errors
- **Phase 4 Sprint & Backtrack**: Added acceleration to 0.01-granularity fine-tune
  - Sprint: doubles step (0.01 → 0.02 → 0.04 → 0.05 max) after 2 consecutive successes
  - Backtrack: resets to 0.01 step on overshoot, retries from last good CRF
  - Dramatically faster while maintaining precision
  - Prevents slow linear 0.01 step exploration
- **Test Compatibility**: Updated test expectations for new constants
  - ULTIMATE_MIN_WALL_HITS: 4 → 15
  - ULTIMATE_REQUIRED_ZERO_GAINS: 20 → 50
  - ABSOLUTE_MIN_CRF: 10.0 → 0.0
- **Missing Field Errors**: Fixed VideoDetectionResult tests with encoder_params and max_b_frames

## [0.10.34] - 2026-03-12

### Added
- **Unified Insight Evaluation Mechanism (3.0 pts)**: Standardized early termination across all search phases based on quality stagnation.
  - **Integer-Level Quality Tracking**: Now specifically monitors for integer improvements in VMAF-Y and PSNR-UV (ignoring decimal fluctuations).
  - **10-Sample Confirmation Window**: Replaces immediate adoption with a mandatory 10-iteration exploration. Each sample without integer quality gain adds 0.3 to the "Insight Index".
  - **Immediate Discard on Saturation**: The search only terminates (discards further exploration) once the index reaches 3.0, ensuring absolute quality saturation.
- **Improved Phase 3 Persistence**: Removed legacy SSIM plateau logic in favor of the high-fidelity VMAF/PSNR insight system.

## [0.10.33] - 2026-03-12

### Added
- **CPU Fine-Tune Sprint & Backtrack**: Implemented an accelerated search algorithm for Phase 3 (Downward Search).
  - **Sprint**: Doubles the CRF step (0.1 → 0.2 → 0.4...) on successful compression to rapidly find the quality ceiling.
  - **Backtrack**: Immediately reverts to the last known good CRF and resets step to 0.1 upon overshooting, ensuring precision without sacrificing speed.
- **Enhanced UI Aesthetics**: Fully colorized Phase headers, Wall Hit warnings, and search results using a unified ANSI color scheme (Success=Green, Warning=Yellow, Failure=Red, Value=Cyan).
- **Single-Line Failure Diagnostics**: Re-engineered the `VIDEO STREAM COMPRESSION FAILED` warning into a concise, professional single-line format with visual separators and localized size units (KB/MB).

### Changed
- **Absolute Quality Freedom (Extreme Mode)**: Removed all artificial CRF barriers for high-fidelity sources.
  - Lowered `ABSOLUTE_MIN_CRF` and `EXPLORE_DEFAULT_MIN_CRF` to **0.0**.
  - Relaxed AV1 minimum CRF clamp from 15.0 to **0.0**.
  - Extended HEVC maximum CRF range to 51.0 for edge-case compatibility.
- **Smart Boundary Awareness**: Updated all search phases to use dynamic `search_floor` (0.0 in Ultimate Mode) instead of legacy hardcoded minimums.

### Fixed
- **Size Tolerance Discrepancy**: Fixed a critical logic error where `conversion_api.rs` would fail an encode due to video stream growth even when `allow_size_tolerance` (1MB) was enabled.
- **Phase 2 Efficiency**: Optimized Phase 2 (Upward Search) to terminate immediately if a Wall Hit occurs at the minimum step (0.1), preventing redundant iterations.

## [0.10.32] - 2026-03-12

### Added
- **Sticky Quality Insights**: Failure credibility no longer resets on minor (decimal-level) quality fluctuations. Once a "Non-Viability Insight" is gained, it persists until a full recovery above the quality gate.
- **Extreme Saturation Depth**: Increased `ULTIMATE_REQUIRED_ZERO_GAINS` to **50 consecutive samples**. This ensures the search firmly hits the "Physical Red Line" (Size Wall) for maximum archival quality.
- **Enhanced Loop Logic**: Increased total iteration limits to 200 to accommodate deeper saturation searches.

## [0.10.31] - 2026-03-12

### Added
- **Credibility-Driven Abort Mechanism**: Replaced count-based fast-fail with a weighted "Failure Credibility Index" (threshold 3.0, +0.3 per low-quality insight).
- **Unified 30-step Saturation**: Consolidated all saturation logic into a mandatory 30-step verification for Ultimate Mode.

## [0.10.30] - 2026-03-12 (Internal Release)
- Preliminary logic cleanup for wall detection and metric caching.

## [0.10.29] - 2026-03-12

### Added
- **Ultimate 'Dead-Wall' Detection**: Intelligent fast-fail for downward search paths.
  - If video quality is already below mandatory thresholds (VMAF 93 / UV 38) and exhibits saturation (3 consecutive zero-gains), the search aborts immediately.
  - Prevents wasting performance on up to 27 redundant iterations when a "Quality Gate" failure is statistically inevitable.
- **Enhanced Ceiling Verification**: Ceiling checks now strictly validate both VMAF-Y and PSNR-UV components.

## [0.10.28] - 2026-03-12

### Added
- **Noise-Resistant Wall Detection**: Introduced a mandatory **10-sample confirmation window** for the "Ultimate Wall" (God Zone: VMAF > 98 / PSNR-UV > 48).
  - Effectively filters out VMAF/PSNR measurement noise and encoder jitter.
  - Prevents early stopping bias by ensuring the quality ceiling is statistically significant.
  - New UI indicator: `[SATURATED X/10]` shows the confirmation progress in purple.

### Changed
- **Total Quality Awareness**: Standardized quality gate checks across both upward (Fast-Fail) and downward (Ceiling) search paths.

## [0.10.27] - 2026-03-12

### Changed
- **Ultimate Saturation Depth**: Increased `ULTIMATE_REQUIRED_ZERO_GAINS` from 20 to **30 consecutive samples** to ensure absolute "Domain Wall" saturation for high-fidelity archival.
- **Refined Quality Fast-Fail**: Upgraded the early-exit logic in Phase 2 Upward Search with a **3-sample confirmation counter**. 
  - Prevents premature aborts due to transient quality dips.
  - Only terminates the search if 3 consecutive CRF steps fail to meet the Phase III quality gate (VMAF 93.0 / PSNR-UV 38.0).

## [0.10.26] - 2026-03-11

### Added
- **Ultimate Mode: Multi-Metric Wall Detection**: In Ultimate mode, the "CRF Wall" detection now uses a combination of **VMAF (Y)** and **PSNR (UV)** instead of relying solely on SSIM-ALL saturation.
  - Detects absolute quality ceilings (VMAF > 98 or PSNR-UV > 48) to prevent wasted bits when perceptual and chroma saturation is reached.
  - Provides detailed feedback: `📊 ULTIMATE WALL DETECTED: VMAF-Y=XX.XX, PSNR-UV=XX.XX`.
- **Loud & Visible Fallback System**: Introduced a highly visible, ANSI-colored warning system for when precise metadata is unavailable and heuristics must be used.
  - Warnings now include the **full filename** for immediate troubleshooting.
  - Multi-tier alerts: Yellow for standard fallbacks, Red for critical detection failures.
- **Enhanced Heuristic Engine (v2)**: Revolutionized image quality estimation when bitstream parsing fails:
  - **Efficiency-Weighted BPP**: Integrated format-specific multipliers (AVIF/HEIC 3.0x, WebP 1.5x) to reflect superior modern compression efficiency.
  - **Texture-Aware Compensation**: Quality estimates are now dynamically adjusted based on image entropy (texture complexity).
- **Premium UI Enhancements**: Upgraded terminal aesthetics with double-line box drawing, new high-fidelity symbols (💠, 🥇, 🛡️), and improved result summary banners.

### Changed
- **Unified 1MB Size Tolerance**: Implemented a mandatory 1MB (`1,048,576 bytes`) size increase tolerance across all video search phases when `--allow-size-tolerance` is enabled.
- **Meme Scoring Rebalance**: Reduced FPS weight to 0.0 to accommodate modern high-frame-rate memes (e.g., Live2D stickers).
- **Dependency Update**: Migrated all workspace dependencies to their latest stable versions (Anyhow 1.0.102, Thiserror 2.0.18, Clap 4.5.60, etc.) and switched from git tags to crates.io for improved stability.
- **Drag & Drop Defaults**: Enabled `--allow-size-tolerance` by default in the macOS drag-and-drop processor script.

### Fixed
- **Strict Metadata Policy**: Eliminated all occurrences of `unwrap_or(24.0)`, `unwrap_or(85)`, and other "irresponsible" silent fallbacks.
- **Code Health & Reliability**: Fixed multiple Clippy warnings, type mismatches in AV1 conversion, and missing fields in unit tests.
- **Scope & Truncation Errors**: Resolved critical scope issues in CRF exploration and ensured long file stability during builds.

## [0.10.25] - 2026-03-11 (Internal Release)
- Preliminary transition to precision-first metadata.
- Internal testing of enhanced heuristic engine.

### Added
- **Absolute-Precision-First Strategy**: Completed the transition to a mandatory precision-first metadata policy. The system now refuses to "cheat" or "fake" critical metadata (FPS, dimensions, quality) through hardcoded defaults.
- **Loud & Visible Fallback System**: Introduced a highly visible, ANSI-colored warning system for when precise metadata is unavailable and heuristics must be used.
  - Warnings now include the **full filename** for immediate troubleshooting.
  - Multi-tier alerts: Yellow for standard fallbacks, Red for critical detection failures.
- **Enhanced Heuristic Engine (v2)**: Revolutionized image quality estimation when bitstream parsing fails:
  - **Efficiency-Weighted BPP**: Integrated format-specific multipliers (AVIF/HEIC 3.0x, WebP 1.5x) to reflect superior modern compression efficiency.
  - **Texture-Aware Compensation**: Quality estimates are now dynamically adjusted based on image entropy (texture complexity).
  - **Animation-Aware BPP**: BPP calculation now correctly accounts for frame count in animated sequences.

### Changed
- **Meme Scoring Rebalance**: Significant update to the GIF/animated image "Meme Score" mechanism:
  - **FPS De-weighting**: Reduced FPS weight to 0.0 to accommodate modern high-frame-rate memes (e.g., Live2D stickers).
  - **Dimension Priority**: Shifted decision weight towards canvas resolution and duration as primary indicators.
- **Unified strict Metadata Parsing**: Standardized `parse_frame_rate` and mandatory dimension checks across `shared_utils`, `vid_av1`, and `vid_hevc`.

### Fixed
- **Silent Metadata Failure**: Eliminated all occurrences of `unwrap_or(24.0)`, `unwrap_or(85)`, and other "irresponsible" silent fallbacks that previously masked detection errors.
- **Unreliable Repeat Rate**: Removed dependence on unreliable repetition metrics that could misidentify source materials as memes.

## [0.10.24] - 2026-03-11

### Added
- **Precise-First Detection Strategy**: Significant refactor of the analysis pipeline to prioritize deterministic metadata over heuristics.
- **Enhanced Video Metadata**: Added `ffprobe` tag extraction and `VideoPrecisionMetadata` to identify original encoder settings (CRF, preset), enabling more accurate quality categorization.
- **GIF Optimization**: Updated GIF source handling to treat them as indexed-lossless, ensuring maximum fidelity when converting to modern formats.
- **HEVC/HEIC Bitstream Analysis**: Replaced hardcoded lossy assumptions for HEIC with real-time bitstream checks for lossless profiles and 4:4:4 chroma.
- **Deterministic Content Selection**: Refined the content classifier to use precise palette and bit-depth indicators for improved Icon/Graphic vs. Photo detection.

## [0.10.23] - 2026-03-11

### Added
- **AV1 Animated Image Parity**: Synchronized `vid_av1` and `img_av1` with their HEVC counterparts to handle animated WebP and JXL inputs efficiently.
  - Implemented `webpmux` pre-extraction for animated WebP to APNG conversion.
  - Added multi-stream validation for animated HEIC/HEIF sequences.
- **AV1 Mathematical Lossless Mode**: Added proper support for `libsvtav1` lossless parameters (`-svtav1-params lossless=1`) within `vid_av1`.

### Changed
- **Delegated AV1 Processing**: Refactored `img_av1/lossless_converter` to delegate all animation-centric processes back to the shared `vid_av1::animated_image` logic, eliminating duplicate definitions and guaranteeing consistent handling.

### Fixed
- **Error Muting in AV1 Conversion**: Fixed a bug inside `vid_av1`'s conversion API where failures returned by `copy_on_skip_or_fail` were quietly swallowed instead of aborting the operation.
- **GIF Fallback Ignorance**: Fixed an issue where animated GIFs were subjected to standard Apple compatibility fallbacks, preventing proper skip preservation.

## [0.10.22] - 2026-03-11

### Added
- **Precision-First Image Quality Detection**: Refactored the quality analysis pipeline to prioritize deterministic metadata extraction over heuristic estimates.
  - **PNG/GIF Palette Detection**: Explicitly parses PNG chunks and GIF Global Color Tables to get exact palette sizes, providing 100% accurate color diversity metrics for indexed formats.
  - **Lossless Determinism**: Implemented deterministic headers checks for WebP (VP8L), HEIC/AVIF (Profile/Chroma), and TIFF (Compression Tag) to accurately identify lossless sources.
  - **High-Bit-Depth Awareness**: Quality heuristics now respect 10-bit+ bit depths extracted directly from headers, adjusting noise and complexity expectations accordingly.
  - **Content Classification Override**: Integrated precise metadata into the content classifier, ensuring PNG-8 and GIF files are correctly identified as Graphics/Icons rather than Photos.

### Changed
- **Unified Analysis Metadata**: Introduced `PrecisionMetadata` struct across `image_detection`, `image_analyzer`, and `image_quality_detector` modules to ensure consistent data propagation.

## [0.10.21] - 2026-03-11

### Changed
- **Standardized 1MB File Size Threshold**: Unified all 1MB size threshold checks across the codebase to exactly `1_048_576` bytes instead of using ambiguous limits (like `1_000_000`, `1000 * 1000`, or `1024 * 1024`).
- **Translation**: Unified log messaging and CLI outputs. Removed all internal Simplified Chinese console messages (e.g. from `pure_media_verifier.rs` and `stream_size.rs`) to full English representation logic for better integration and consistency across regions.

## [0.10.21] - 2026-03-11

### Fixed
- **Ctrl+C Bypass Bug**: Fixed a severe issue where intercepting Ctrl+C failed to suspend active processing tasks. Previously, the confirmation prompt was displayed on a separate background thread without locking or notifying the `rayon` thread pool or global output buffers. Working tasks continued executing (and spamming the UI) while the prompt awaited user input. Now, `ctrlc_guard` explicitly exports its blocking state, intercepting both UI log emissions and core work allocation loops natively, effectively pausing all resource consumption until the user decides.

### Changed
- **Deep UI Modernization & TrueColor Integration**: Revamped terminal aesthetics across the application. Added full RGB 24-bit TrueColor constants (`MFB_Blue`, `MFB_Purple`, `MFB_Pink`, `MFB_Green`) to `modern_ui.rs`.
- **Card-based Terminal Output**: Upgraded static data displays to sophisticated rounded-corner "Card" styles featuring the project's brand color, underline emphasis, and precision spacing.
- **Summary Report Overhaul**: The end-of-batch Summary Report was transformed from a plain ASCII table to a stunning modern UI container, enhancing data legibility with semantic colors (Red, Green, Yellow) that dynamically correspond to the run's success rate and file size reductions.

## [0.10.20] - 2026-03-11

### Fixed
- **Terminal Color Restoration**: Fixed an issue where the terminal output lacked ANSI colors (leaving only black and white text) by ensuring the wrapper script `drag_and_drop_processor.sh` explicitly exports `FORCE_COLOR=1` down to the Rust binaries.
- **Terminal Progress Stats Layout & Color Loss**: Replaced the ugly `\x1b[1A` cursor movement code that previously mangled terminal outputs when piped via `tee`. Global progress statistics are now generated dynamically and embedded as perfectly aligned inline content directly on the success logs (e.g. `XMP: 29✓ Img: 18✓`). ANSI color sequences (`\x1b[1;32m` for reduction, `\x1b[1;33m` for increases) were precisely restored inside string payloads to ensure the bash terminal accurately renders the colors.
- **Image Conversion Summary UX**: Refined the spacing for the final `Images: X OK, Y failed` log block, shrinking the massive 25-space padding gap to align nicely and compactly with the rest of the output.
- **Ctrl+C (SIGINT) Guard Deadlock**: Addressed a fatal bug where the 10-second background thread reading user prompts on Ctrl+C would hang indefinitely in a blocked `read_line` state. The wait thread logic was completely removed in favor of using OS-level `libc::poll` on `STDIN_FILENO` with a 10s timeout, making the UI perfectly responsive.
- **Bash `tee` Output Crash & Linger on SIGINT**: Thoroughly patched terminal pipeline termination handling! Previously, attempting to quit via Ctrl+C failed because the inner execution instances of `tee` silently crashed, and Rust's `130` interrupt code was swallowed. We wrapped all inner `tee` pipes in `(trap '' INT; tee)` buffers, and explicitly programmed the Bash wrapper to listen for `PIPESTATUS[0] -eq 130` on both `img_hevc` and `vid_hevc` invocations to exit reliably. Additionally, an `EXIT` trap was introduced to guarantee the background title bar timer (spinner) destroys itself instead of outliving the script.
- **GIF Apple Compat Log Precision**: Specified formatting strings exactly as requested for fallback actions: `🎞️  GIF [filename] → KEEP GIF` and `🎞️  GIF [filename] probe failed → KEEP GIF`.

## [0.10.19] - 2026-03-10

### Fixed
- **TTY title bar padding causing clear-screen**: The `_tty_title()` function in the drag-and-drop script had thousands of spaces as padding to overwrite previous title content. This padding was leaking into the terminal output stream, causing periodic clear-screen effects and macOS Terminal notification badges
  - **Root cause**: The massive padding string (thousands of spaces) in the OSC escape sequence `\033]0;⏱ %s <spaces>\007` was somehow leaking into stdout/stderr, getting captured by `tee`, and dumped to the terminal
  - **Fix**: Removed all padding from `_tty_title()`. Modern terminals automatically clear the rest of the title bar, so padding is unnecessary
  - **Files modified**: `scripts/drag_and_drop_processor.sh`

- **Ctrl+C confirmation auto-resume not working**: After the 8-second timeout in the Ctrl+C confirmation window, the script would print "Resuming..." but then immediately exit with "Interrupted by user" instead of actually resuming. The root cause was that `read -r -t 8` returns non-zero on timeout, and the original logic treated any non-zero return as "user didn't press y", but didn't distinguish between timeout and actual user input
  - **Root cause**: The `if read -r -t 8 ...` condition was false on timeout (exit code >128), causing the code to fall through to the else branch. But the logic didn't properly check if the user explicitly pressed 'y' - it only checked the read success, not the actual answer
  - **Fix**: Capture the `read` exit code explicitly with `read ... || read_result=$?`, then check both the exit code AND the answer. Only exit if `read_result == 0` (got input) AND `answer == 'y'`. All other cases (timeout, 'n', any other key) resume processing
  - **Files modified**: `scripts/drag_and_drop_processor.sh`

- **Milestone status lines not showing persistently**: Status lines were only shown at intervals (every 5/20/100 merges) instead of on every successful merge
  - **Root cause**: Used `xmp_milestone_interval()` function to control display frequency, causing gaps in visibility during processing
  - **Fix**: Removed interval logic entirely - now emits status line on EVERY XMP merge for persistent display
  - **Impact**: Users can now see continuous progress updates with current statistics on every merge
  - **Files modified**: `shared_utils/src/progress_mode.rs`

- **Ctrl+C guard completely ineffective in Rust processes**: The shell-level Ctrl+C confirmation was bypassed because Rust processes received SIGINT directly and exited immediately
  - **Root cause**: When user presses Ctrl+C, both shell script and Rust process receive SIGINT simultaneously. Even though shell showed confirmation prompt, Rust process already exited
  - **Fix**: Implemented native Rust Ctrl+C handler using `ctrlc` crate with 4.5-minute threshold
    - Before 4.5 min: Ctrl+C exits immediately (unchanged behavior)
    - After 4.5 min: Rust process shows confirmation prompt and waits for user input
    - Press 'y': clean exit with proper cleanup
    - Press 'n' or timeout (8s): resume processing
  - **Impact**: True protection against accidental termination of long-running batch jobs
  - **Files modified**: `Cargo.toml`, `shared_utils/Cargo.toml`, `shared_utils/src/ctrlc_guard.rs` (new), `shared_utils/src/lib.rs`, `img_hevc/src/main.rs`, `img_av1/src/main.rs`

- **Milestone status lines too verbose and not narrow-screen friendly**: The inline milestone format was too long with excessive spacing: `                       📊                          XMP merge: 80 OK   Images: 81 OK`
  - **Root cause**: Used column 120 positioning and included 25 spaces of padding from `STATS_PREFIX_PAD`
  - **Fix**: Redesigned milestone format to be compact and beautiful:
    - Use `│` separator instead of excessive spacing
    - Shortened text: "XMP: 80✓  Img: 81✓" instead of "XMP merge: 80 OK   Images: 81 OK"
    - Use `\x1b[999C\x1b[60D` (move to end, then back 60 chars) to align 📊 with ✅
    - Format: `  │ 📊 XMP: 80✓  Img: 81✓` (compact, narrow-screen friendly)
  - **Files modified**: `shared_utils/src/progress_mode.rs`

### Removed
- **Unused milestone interval functions**: Removed `xmp_milestone_interval()` and `image_milestone_interval()` functions since milestones are now shown on every merge
  - **Files modified**: `shared_utils/src/progress_mode.rs`

## [0.10.18] - 2026-03-10

### Fixed
- **Periodic screen clearing / terminal notification badges during batch processing**: Progress bar was created before `enable_quiet_mode()`, causing indicatif to render to stderr every 50ms
  - **Root cause**: `UnifiedProgressBar::new()` called before `enable_quiet_mode()` → bar created in non-quiet mode → rendered updates to stderr every 50ms → caused screen flicker and macOS Terminal notification badges when terminal was in the background
  - **Fix**: Swapped order — `enable_quiet_mode()` first, then create bar. Additionally removed all `pb` usage (creation, `set_position`, `set_message`, `finish_with_message`) from `img_hevc` and `img_av1` batch loops entirely since the title-bar spinner replaces them
  - **Files modified**: `img_hevc/src/main.rs`, `img_av1/src/main.rs`

### Added
- **File-type emoji prefixes on per-file log lines**: `🖼️` for images, `🎬` for videos
  - Format: `🖼️ [Cache_4ac28036…jpg] JPEG lossless transcode: size reduced 27.5% ✅`
  - Emoji is added before the `[filename]` tag; message body alignment is unchanged
  - **Files modified**: `shared_utils/src/progress_mode.rs` (new `file_type_emoji()` helper, updated `format_log_line()`)

### Removed
- **`--lossless` CLI flag from all 4 binaries** (`img-hevc`, `img-av1`, `vid-hevc`, `vid-av1`): Dead CLI surface — never passed by the drag-and-drop script. The internal lossless conversion logic remains intact: lossless sources are still converted losslessly by default (JPEG→JXL lossless transcode, lossless PNG→JXL, lossless animated→AV1 CRF 0). The flag only forced *all* conversions to mathematical lossless mode (very slow), which was never used in practice
  - Removed from CLI arg definitions in `Commands::Run` enum
  - Removed from `AutoConvertConfig` / `ConversionConfig` structs
  - Removed conditional branches — always use smart quality matching path
  - **Files modified**: `img_hevc/src/main.rs`, `img_av1/src/main.rs`, `vid_hevc/src/main.rs`, `vid_av1/src/main.rs`

- **`Simple` subcommand from `vid-hevc` and `vid-av1`**: This mode forced all videos to a fixed CRF (HEVC CRF 18 / AV1 mathematical lossless), bypassing smart quality matching. Never used by the drag-and-drop script
  - Removed `Commands::Simple` enum variant and its match arm
  - **Files modified**: `vid_hevc/src/main.rs`, `vid_av1/src/main.rs`

- **Obsolete `create_conditional_progress()` helper**: Removed from `progress_mode.rs`
  - **Files modified**: `shared_utils/src/progress_mode.rs`

### Notes
- **`--force` flag** (kept): Controls whether already-processed files and existing output files are overwritten. Used throughout the conversion pipeline. Essential for re-running conversions
- **Behavior change**: With `--lossless` removed, animated GIFs/WebP/APNG always use smart quality matching. Static images still use lossless conversion paths unchanged

## [0.10.17] - 2026-03-10

### Fixed
- **Memory limit exceeded for very large JPEGs (e.g. 99MB `mmexport1732810380466.jpeg`)**: The `image` crate's default memory allocation ceiling (~512MB) was too low to decode large JPEGs from high-resolution cameras. A 99MB JPEG can expand to ~800MB+ of raw pixel data when fully decoded
  - **Root cause**: `image::open()` uses conservative default `Limits::default()` which enforces a ~512MB `max_alloc` ceiling. The raw decoded pixels of a 100MP+ JPEG easily exceed this
  - **Fix**: Replaced all bare `image::open()` / `ImageReader::open()` calls with a shared `open_image_with_limits()` helper that raises `max_alloc` to 2GB. This covers 100MP+ images at full color depth (e.g. 300MP × 4 bytes = ~1.2GB max) while still rejecting pathologically large malicious inputs above 2GB
  - **Memory safety**: The 2GB limit is a ceiling, not a reservation. Normal images (1–20MP) still use only the memory their pixels actually require (typically 4–80MB). The limit only matters for edge-case 100MP+ images, which are rare and legitimate
  - **Files modified**: `shared_utils/src/image_detection.rs` (new `pub open_image_with_limits()`), `shared_utils/src/image_analyzer.rs`, `img_hevc/src/main.rs`

### Added
- **Ctrl+C confirmation guard for long-running jobs**: Pressing Ctrl+C after 4.5 minutes of processing now shows a confirmation prompt before exiting, preventing accidental termination of large batch jobs
  - Before 4.5 min: Ctrl+C exits immediately (unchanged behavior)
  - After 4.5 min: Shows `Confirm exit? [y/N] (auto-resume in 8s)`
    - Press `y`/`Y`: clean exit (stops spinner, restores cursor, shows elapsed time)
    - Press `n`/`N`, any other key, or no input within 8 seconds: resumes processing
  - Reads confirmation from `/dev/tty` so it works even when stdin is piped
  - **Files modified**: `scripts/drag_and_drop_processor.sh`

## [0.10.16] - 2026-03-10

### Fixed
- **Per-file success lines silent in batch mode**: `[filename] message ✅` lines were suppressed during parallel batch processing because `enable_quiet_mode()` routed them to the log file only, not the terminal
  - **Root cause**: The `is_quiet_mode()` branch was originally added to prevent per-file lines from colliding with the indicatif progress bar. Since the progress bar was moved to the terminal title bar (OSC escape), there is no longer anything in the terminal content area to collide with
  - **Fix**: Removed the quiet-mode branch in `img_hevc` and `img_av1` — always emit per-file result lines via `log_eprintln!` (→ `emit_stderr`) regardless of quiet mode
  - **Files modified**: `img_hevc/src/main.rs`, `img_av1/src/main.rs`

## [0.10.15] - 2026-03-10

### Fixed
- **Script syntax error on double-click (line 301)**: `bash -n` revealed a missing closing quote on line 218 in `draw_header()` — `echo -e "..." ` was missing the trailing `"`, causing bash to continue parsing the string literal across subsequent lines until it hit the `(` at line 301 and reported `syntax error near unexpected token '('`
  - **Root cause**: A single missing `"` at the end of an `echo -e` line in `draw_header()` caused bash to treat everything up to the next `"` (83 lines later) as a string continuation
  - **Fix**: Added the missing closing `"` on line 218
  - **Files modified**: `scripts/drag_and_drop_processor.sh`

- **Inconsistent clear-screen behavior after build**: Script sometimes cleared a large block of build output before showing the mode-selection menu, sometimes didn't
  - **Root cause**: `_main()` called `clear_screen` at the very start, before `check_tools` (which runs the build). When the build was cached/fast it produced no output and the clear was harmless; when the build printed compilation output, `clear_screen` ran first (clearing nothing visible yet), then build output filled the screen, and then `select_mode()` called `clear_screen` again — this second clear was the one users saw, making behavior appear inconsistent
  - **Fix**: Removed the premature `clear_screen` at the top of `_main()`. `select_mode()` already clears the screen at the start of its menu loop, ensuring a consistent clean display every time
  - **Files modified**: `scripts/drag_and_drop_processor.sh`

## [0.10.14] - 2026-03-10

### Changed
- **Beautiful log output with refined emoji usage**: Multiple iterations of log formatting improvements for better aesthetics, clarity, and intent
  - **Single-line format with visual separators**: Replaced multi-line cluttered logs with clean single-line format using `│` separators for better visual organization
  - **Precise emoji control**: Implemented exactly 4 emojis per log section (1 left, 3 right maximum) with logical consistency
    - Success: 1 `✅ QUALITY GATE` + 3 `✅` metrics = 4 emojis
    - Failure: 1 `❌ QUALITY GATE` + 3 `❌` metrics = 4 emojis
    - Partial failure: 1 `❌ QUALITY GATE` + mixed `✅❌` metrics = 2-4 emojis
  - **Emoji positioning**: Moved primary emoji to QUALITY GATE position for meaningful quality validation indication
  - **Logical emoji consistency**: ✅ for success/pass, ❌ for failure/fail - no contradictory emoji states

### Improved
- **Visual hierarchy and readability**: Enhanced log structure with clear indentation, proper spacing, and consistent formatting
- **Information density**: Balanced between comprehensive detail and visual clarity - important information stands out without clutter
- **Professional terminal display**: Optimized for terminal viewing with appropriate use of emojis, separators, and spacing
- **Clear intent**: Log messages now clearly convey their purpose and status without ambiguity

### Technical Details
- **Files modified**: `shared_utils/src/video_explorer/gpu_coarse_search.rs`, `vid_hevc/src/conversion_api.rs`, `vid_hevc/src/animated_image.rs`, `vid_av1/src/conversion_api.rs`
- **Log format evolution**: Progressed from multi-line → forced single-line → beautiful single-line → emoji-controlled → logically consistent
- **Emoji strategy**: Balanced visual appeal with functional clarity, avoiding emoji abuse while maintaining important visual cues
- **Separator choice**: Used `│` (pipe) separators for clean visual division without overwhelming the display

### Fixed
- **Terminal `Running: Xs` spinner text fusing into binary output lines**: The bash spinner writes `\r Running: Xs` to `/dev/tty` every 0.15s while binaries write progress to stderr on the same terminal, producing fused lines like `   | Running: 04s     [file] ✓ CRF 28.3:` and leftover spinner text after processing
  - **Root cause**: Spinner and binary both write to the terminal content area concurrently. `\r` moves cursor to column 0 without erasing, so binary output appends directly after spinner text. Any subsequent newline permanently commits the fused line to scrollback — no amount of pause/resume/clear can prevent this
  - **Fix**: Moved spinner display from terminal content area (`\r` writes) to the **terminal title bar** (OSC escape `\033]0;...\007`). The title bar is completely isolated from the content area, making collision fundamentally impossible. Binary output (`tee /dev/stderr`) flows normally in the terminal content with zero interference
  - **Result**: Running time visible in terminal tab/title bar, binary progress visible in content area, no residue anywhere
  - **Files modified**: `scripts/drag_and_drop_processor.sh`

- **Clippy: `format!` in `format!` args (14 warnings)**: Inlined nested `format!()` calls for ANSI color strings into their outer `format!()` calls across all affected crates
  - `shared_utils/src/conversion.rs` (4 occurrences)
  - `img_hevc/src/conversion_api.rs` (2 occurrences)
  - `img_av1/src/conversion_api.rs` (2 occurrences)
  - `vid_hevc/src/animated_image.rs` (6 occurrences — HEVC, Lossless HEVC, GIF Apple Compat)
  - Workspace now compiles with zero clippy warnings at `--release` profile

## [0.10.13] - 2026-03-10

### Changed
- **Statistics lines now use 📊 emoji instead of `[Info]` tag**: The `[Info]` prefix on periodic stats lines (e.g. `XMP merge: 253 OK   Images: 200 OK`) was misleading — it resembles a log severity level, but these lines are counters/statistics, not informational log messages. Replaced with a `📊` emoji for clarity
- **Visual separation for statistics lines**: Periodic mid-run stats lines now have a leading blank line (`\n`) before them so they stand out clearly when interleaved with per-file progress output, avoiding the previous ugly inline merging

## [0.10.12] - 2026-03-10

### Fixed
- **Terminal colors not appearing when launched via drag-drop script or app**: Root cause was `console::style()` stripping ANSI codes when stderr is not a TTY (which is always the case when piped through `tee /dev/tty | tee -a logfile`)
  - **Fix**: Replaced all `console::style(...)` color calls with raw ANSI escape codes (`\x1b[1;32m`, `\x1b[1;33m`, etc.) so color codes are embedded in the string unconditionally
  - **Fix**: Rewrote `emit_stderr()` to use `writeln!(std::io::stderr(), ...)` directly instead of routing through `tracing::info!`, bypassing tracing-subscriber's own TTY detection which also stripped colors
  - **Fix**: Added ANSI stripping in `write_to_log()` so file logs remain plain text even though the in-memory strings now carry raw escape codes
  - **Result**: Colors now correctly flow through the `2>&1 | tee /dev/tty` pipe chain and appear in the terminal for all launch modes

- **Removed stray Chinese comments in `img_hevc/src/main.rs` and `img_av1/src/main.rs`**: Two inline comments remained in Chinese after the English-only conversion; now removed

## [0.10.11] - 2026-03-09

### Changed
- **App and script fully in English**: Converted all Chinese UI text in the macOS app wrapper and drag-and-drop script to English
  - App dialogs: "Select folder to process", "Will optimize the following folder", "Start Optimization", "Cancel", timeout alerts
  - App wrapper comments fully in English
  - All user-facing strings are now English-only

- **Colorized terminal output for conversion results**: Key outcome text is now color-coded for immediate visual feedback
  - `size reduced X%` → **green bold** (success, space saved)
  - `size increased X%` → **yellow bold** (accepted but no size gain)
  - Size-check rejection messages: increased amount in **yellow bold**
  - Deleted output notifications: reason text in **yellow bold**
  - Applied across all converters: `shared_utils`, `img_hevc`, `img_av1`, `vid_hevc` (HEVC, Lossless HEVC, GIF Apple Compat)

- **Standardized logging macros across all binaries**: Replaced raw `eprintln!`/`println!` with `shared_utils::log_eprintln!` in `img_hevc/src/main.rs`, `img_av1/src/main.rs`, `vid_hevc/src/main.rs`
  - Warning messages use `console::style(...).yellow()` for consistent visual identity
  - Error messages route through `log_auto_error!` for automatic severity classification
  - All output now captured in file logs (previously stdout-only calls were invisible to logs)

- **Intermediate conversion steps route through emit_stderr**: WebP→APNG, JXL→APNG, Stream→APNG success messages in `vid_hevc` now use `progress_mode::emit_stderr` so they appear in file logs

## [0.10.10] - 2026-03-09

### Added
- **Enhanced error logging system**: Critical and rare error detection with color-coded severity levels
  - **Motivation**: Early detection of rare bugs (pipeline broken, metadata loss, upstream tool errors) to prevent data/quality loss
  - **Error severity levels**:
    - 🚨 **CRITICAL**: Data loss, corruption, truncation (red bold)
    - ⚠️ **RARE ERROR**: Unexpected upstream tool failures, assertion failures (yellow bold)
    - 📋 **METADATA LOSS**: Missing or stripped metadata (magenta bold)
    - 🔧 **PIPELINE BROKEN**: Broken pipe, connection reset, unexpected EOF (cyan bold)
    - 🔺 **UPSTREAM ERROR**: FFmpeg/ImageMagick/cjxl unexpected behavior (yellow bold)
  - **Auto-classification**: Errors are automatically classified by pattern matching
  - **New macros**: `log_critical!`, `log_rare_error!`, `log_metadata_loss!`, `log_pipeline_broken!`, `log_upstream_error!`, `log_auto_error!`
  - **Applied to**:
    - FFprobe image2 demuxer pattern matching failures (rare error)
    - cjxl non-zero exit codes (upstream error)
    - Pipeline process wait failures (pipeline broken)
  - **Impact**: Rare bugs now highly visible in both terminal (colored) and file logs, enabling faster bug detection and fixes
  - **Files added**: `shared_utils/src/error_logging.rs`
  - **Files modified**: `shared_utils/src/lib.rs`, `shared_utils/src/ffprobe_json.rs`, `shared_utils/src/jxl_utils.rs`

- **Comprehensive file logging**: Success/failure messages now written to file logs
  - **Root cause**: Success messages used `println!()` (stdout) instead of logging macros, so file logs were incomplete
  - **Fix**: Changed `println!()` to `log_eprintln!()` to capture all output in file logs
  - **Impact**: File logs are now the most comprehensive record, including all media processing results
  - **Files modified**: `img_hevc/src/main.rs`, `img_av1/src/main.rs`

- **App mode log merging**: Automatic log consolidation when running via double-click
  - **Feature**: When launched via macOS app, automatically merges 3 separate logs into single `merged_*.log`
  - **Merged logs**: Drag-drop script + Image processing + Video processing
  - **Detection**: Uses `FROM_APP` environment variable set by app wrapper
  - **Impact**: Easier log review for app users, single comprehensive file
  - **Files modified**: `scripts/drag_and_drop_processor.sh`, `Modern Format Boost.app/Contents/MacOS/Modern Format Boost`

## [0.10.9] - 2026-03-09

### Changed
- **Size tolerance logic**: Changed from percentage-based (1%) to KB-level (< 1MB) tolerance
  - **Rationale**: Percentage-based tolerance was unfair to small files (1% of 10KB = 100 bytes is too strict)
  - **New behavior**: Accept output if size increase < 1MB, regardless of file size
  - **Impact**: More reasonable tolerance for small files while maintaining strictness for large files
  - **Display**: Size changes now shown in both KB/MB and percentage for better clarity

- **Compress and tolerance coordination**: Compress mode now respects tolerance setting
  - **Previous**: Compress always rejected output ≥ input (ignored tolerance completely)
  - **Current**: Compress + tolerance enabled = accept if increase < 1MB
  - **Behavior matrix**:
    | compress | tolerance | increase | result |
    |----------|-----------|----------|--------|
    | true | true | < 1MB | ✅ accept |
    | true | true | ≥ 1MB | ❌ reject |
    | true | false | > 0 | ❌ reject |

### Fixed
- **Comprehensive ImageMagick fallback logging**: Enhanced error handling and retry logic for JXL conversion fallback pipeline
  - **Root cause**: ImageMagick fallback had silent failures and incomplete retry logic
  - **Issues fixed**:
    1. Attempt 2+ success/failure had no log output (silent execution)
    2. `is_grayscale_icc_cjxl_error` too strict (required exact string match)
    3. 8-bit source retry logic nested incorrectly
    4. No final fallback for general failures
  - **Improvements**:
    - Added comprehensive logging for all attempts (1-4) with colored ✅/❌ status
    - Enhanced `is_grayscale_icc_cjxl_error` with relaxed matching (libpng warning + grayscale + icc indicators)
    - Restructured retry flow for better 8-bit vs 16-bit handling
    - Added final fallback attempt with -strip for edge cases
  - **Example output**:
    ```
    🔄 Attempt 1: Default (16-bit, preserve metadata)
    ❌ Attempt 1 failed (magick: ✓, cjxl: ✗)
    🔄 Attempt 2: Grayscale ICC fix (-strip, 16-bit)
    ✅ Attempt 2 succeeded
    ```
  - **File modified**: `shared_utils/src/jxl_utils.rs`

- **Fixed compress mode to respect tolerance setting**: Compress mode now honors `allow_size_tolerance` flag
  - **Root cause**: Compress mode always rejected output ≥ input, completely ignoring tolerance setting
  - **Impact**: Files with KB-level size increase (< 1MB) were incorrectly rejected even with tolerance enabled
  - **Example**: 238KB → 420KB (+177KB) was rejected, but should be accepted (< 1MB tolerance)
  - **New behavior**: 
    - `compress=true` + `tolerance=true`: accept if increase < 1MB ✅
    - `compress=true` + `tolerance=false`: reject if output ≥ input ❌
  - **File modified**: `shared_utils/src/conversion.rs`

- **Changed size tolerance from percentage to KB-level**: Fixed logic bug where percentage-based tolerance was unfair to small files
  - **Root cause**: 1% tolerance meant 100 bytes for 10KB files (too strict) but 100KB for 10MB files (reasonable)
  - **New logic**: KB-level tolerance - accept if size increase < 1MB (regardless of file size)
  - **Examples**:
    - 10KB → 1000KB (990KB increase) ✅ accepted
    - 10KB → 1025KB (1015KB = 1MB+ increase) ❌ rejected
    - 10MB → 11MB (1MB increase) ❌ rejected
  - **Impact**: Fairer tolerance for all file sizes, especially small files
  - **Display**: Size changes now shown in KB/MB units instead of just percentages
  - **Files modified**: `shared_utils/src/conversion.rs`, `shared_utils/src/conversion_types.rs`

- **Enhanced size check logging and copy-on-fail feedback**: Improved visibility of file deletion and copy operations
  - **Root cause**: When output files were deleted due to size increase, logs only appeared in `--verbose` mode
  - **Impact**: Users couldn't see why conversions were skipped or where original files were copied
  - **Fix**: 
    - Always log file deletion with clear reason (not just in verbose mode)
    - Show explicit "Original copied to: <path>" message when files are copied to output directory
    - Display size comparison for all skip scenarios
  - **Example output**:
    ```
    🗑️  JPEG (Sanitized) -> JXL output deleted: larger than input by 76.1% (tolerance: 1.0%)
    📊 Size comparison: 238543 → 419973 bytes (+76.1%)
    📋 Original copied to: /tmp/test_output/IMG_6171_副本.jpeg
    ```
  - **File modified**: `shared_utils/src/conversion.rs` (`check_size_tolerance` function)

- **FFprobe image2 demuxer pattern matching issue**: Fixed critical bug where image files with `[` `]` in filenames failed to process
  - **Root cause**: FFprobe's image2 demuxer interprets `[` `]` as sequence patterns (e.g., `image[001-100].jpg`)
  - **Example**: File `FB55N[I_R{KE)K}I141L%8V.jpeg` would fail with "Could find no file with path ... and index in the range 0-4"
  - **Fix**: Added automatic fallback with `-pattern_type none` when image2 demuxer pattern error is detected
  - **Impact**: All image files with special characters in names can now be processed correctly
  - **File modified**: `shared_utils/src/ffprobe_json.rs`

- **Silent ffprobe errors**: Fixed bug where ffprobe errors were silently suppressed due to `-v quiet` flag
  - **Root cause**: Using `-v quiet` prevented stderr capture, making fallback detection impossible
  - **Fix**: Changed all ffprobe calls to use `-v error` to capture error messages for proper fallback handling
  - **Impact**: Better error diagnostics and proper fallback behavior
  - **Files modified**: `shared_utils/src/ffprobe_json.rs`, `shared_utils/src/image_analyzer.rs`

- **Missing success output**: Fixed bug where successful conversions showed no output unless `--verbose` flag was used
  - **Root cause**: Success messages were wrapped in `verbose_log!` macro
  - **Fix**: Always display success messages with ✅ emoji, regardless of verbose mode
  - **Impact**: Users now see clear feedback when conversions succeed
  - **Files modified**: `img_hevc/src/main.rs`, `img_av1/src/main.rs`

- **Misleading quality check log messages**: Fixed logical paradox in quality verification messages
  - **Root cause**: In Ultimate Mode, `ms_ssim_score` stores VMAF-Y (0-1 scale), not MS-SSIM score
  - **Example**: Log showed "MS-SSIM TARGET FAILED: 0.9939 < 0.90" which is mathematically false
  - **Reality**: Quality gate can fail due to CAMBI (banding) or PSNR-UV (chroma) even with high VMAF (99.39%)
  - **Fix**: Changed messages to generic "QUALITY TARGET FAILED (score: X.XXXX)" without misleading comparison
  - **Impact**: Clear diagnostic messages that don't confuse users with apparent logical contradictions
  - **File modified**: `vid_hevc/src/conversion_api.rs`

- **Timestamp verification diagnostics**: Improved error handling for filesystem timestamp sync failures
  - **Root cause**: macOS filesystem protection or network/cloud mounts can prevent timestamp modification
  - **Example**: "⚠️ Failed to restore directory timestamps" appeared without context
  - **Fix**: Added failure counters and summary message explaining possible causes
  - **Impact**: Users now see clear message: "TIMESTAMP VERIFICATION: X/Y directories failed (possible filesystem protection or network mount)"
  - **File modified**: `shared_utils/src/metadata/mod.rs`

- **FFprobe failures on special characters in filenames**: Fixed critical bug where ffprobe failed on filenames containing `[`, `]`, `{`, `}`, `%` characters
  - **Root cause**: ffprobe interprets these characters as URL glob patterns or format specifiers, causing "non-zero exit" errors
  - **Example**: File `FB55N[I_R{KE)K}I141L%8V.jpeg` would fail with "FFPROBE FAILED: non-zero exit"
  - **Fix**: Added `--` separator before file path arguments in all ffprobe invocations to prevent interpretation as options/patterns
  - **Impact**: All files with special characters in names can now be processed correctly
  - **Files modified**: 
    - `shared_utils/src/ffprobe_json.rs` (extract_color_info - user files, direct trigger)
    - `shared_utils/src/stream_size.rs` (try_ffprobe_extraction - user files)
    - `shared_utils/src/video_explorer.rs` (get_input_duration - user files)
    - `shared_utils/src/image_analyzer.rs` (3 locations - temp files)
    - `shared_utils/src/image_detection.rs` (frame count check - temp files)

- **x265 calibration failures on empty y4m samples**: Fixed rare bug where x265 dynamic calibration would fail with "unable to open input file"
  - **Root cause**: For certain videos, ffmpeg extraction exits with code 0 (success) but writes empty y4m file (0 bytes), possibly due to no decodable frames in first 15 seconds or codec mismatch
  - **Example**: Video `6946418393937362319.mp4` failed all 3 CRF calibration attempts (20/18/22) with misleading x265 error
  - **Fix**: Added file size validation after ffmpeg extraction - skip CRF attempt if y4m file is empty
  - **Impact**: Clear diagnostic message instead of misleading x265 error; graceful fallback to GPU-only calibration
  - **File modified**: `shared_utils/src/video_explorer/dynamic_mapping.rs`

### Technical Details
- **FFprobe `--` separator**: The `--` argument tells ffprobe "all following arguments are file paths, not options"
  - Prevents `[` `]` from being interpreted as glob patterns
  - Prevents `{` `}` from being interpreted as format specifiers
  - Prevents `%` from being interpreted as format codes
  - All user file paths now use: `.arg("--").arg(safe_path_arg(path).as_ref())`
- **Y4M validation**: Added guard after ffmpeg extraction:
  ```rust
  let y4m_size = fs::metadata(&temp_input).map(|m| m.len()).unwrap_or(0);
  if y4m_size == 0 {
      eprintln!("❌ Extracted y4m sample is empty for CRF {:.1} (ffmpeg exited 0 but wrote nothing); skipping", anchor_crf);
      continue;
  }
  ```
- **Error messages**: Improved diagnostics for both issues - clear indication of root cause instead of misleading downstream errors

## [0.10.8] - 2026-03-09

### Fixed
- **Multi-stream AVIF/HEIC stream selection bug**: Fixed critical bug where multi-stream animated files selected wrong stream
  - **Root cause**: `probe_video()` returned enumerate index instead of actual stream index from JSON
  - **Impact**: Animated AVIF/HEIC files with multiple streams (thumbnail + animation) only converted first frame instead of all frames
  - **Fix**: 
    - Modified `probe_video()` to use actual stream `index` field from ffprobe JSON
    - Added multi-stream detection in `convert_to_hevc_mp4_matched()`
    - Convert multi-stream AVIF/HEIC to APNG before processing (preserves all frames)
  - **Testing**: Verified 3-frame AVIF (GBR and YUV) converts correctly to MOV (3 frames, 0.3s, 10fps)
  - **Files modified**: `shared_utils/src/ffprobe.rs`, `vid_hevc/src/animated_image.rs`

### Technical Details
- `probe_video()` now correctly extracts `stream["index"]` from JSON instead of using enumerate index
- For multi-stream AVIF/HEIC in `convert_to_hevc_mp4_matched()`:
  - Detect multiple video streams using ffprobe
  - Convert correct stream (with most frames) to APNG using FFmpeg
  - Process APNG through explore functions (ensures correct frame count)
- APNG duration detection now works via `-count_frames` and `nb_read_frames` fallback
- Temporary APNG files are automatically cleaned up

### Testing Results
- ✅ AVIF GBR (3 frames) → MOV: 3 frames, 0.3s, 10fps, HEVC, YUV420p
- ✅ AVIF GBR (3 frames) → GIF: 3 frames, 0.3s, 10fps
- ✅ AVIF YUV (3 frames) → MOV: 3 frames, 0.3s, 10fps, HEVC, YUV420p
- ✅ WebP (3 frames) → MOV: 3 frames, 0.3s, 10fps, HEVC
- ✅ WebP (3 frames) → GIF: 3 frames, 0.3s, 10fps

## [0.10.7] - 2026-03-09

### Fixed
- **WebP frame extraction and timing**: Complete rewrite of WebP → video conversion pipeline
  - **Root cause**: ImageMagick's WebP → APNG conversion was unreliable (frame duplication, incorrect timing)
  - **Fix**: Implemented proper WebP frame extraction using `webpmux` tool
    1. Use `webpmux -info` to get accurate frame count and duration from WebP metadata
    2. Use `webpmux -get frame N` to extract each frame as WebP
    3. Convert each WebP frame to PNG using FFmpeg
    4. Create APNG from PNG sequence with correct frame rate using FFmpeg
  - **Impact**: WebP files now convert with exact frame count and timing (e.g., 3 frames @ 100ms/frame = 0.3s, not 9 frames @ 40ms/frame = 0.36s)
  - **Requirement**: `webpmux` tool must be installed (part of libwebp package)
  - **Files modified**: `vid_hevc/src/animated_image.rs` (all three conversion functions)

- **APNG duration detection**: Fixed ffprobe inability to read APNG duration metadata
  - **Root cause**: APNG format doesn't store duration in container metadata, requires frame counting
  - **Fix**: Added `-count_frames` parameter to ffprobe and use `nb_read_frames` for frame count
  - **Impact**: APNG files (including temporary APNG from WebP) now have correct duration detection
  - **Files modified**: `shared_utils/src/video_explorer/precheck.rs`

### Technical Details
- `extract_webp_to_apng()` function now:
  - Parses WebP metadata using `webpmux -info` for accurate frame count and duration
  - Extracts each frame as WebP (not PNG) using `webpmux -get frame N`
  - Converts WebP frames to PNG using FFmpeg (handles WebP decoding properly)
  - Creates APNG using FFmpeg with `apng` codec (not `png` codec) and `-r` parameter for frame rate
- `run_precheck_ffprobe()` now includes `-count_frames` and `nb_read_frames` in show_entries
- `parse_duration_from_precheck_json()` now falls back to `nb_read_frames` when `nb_frames` is 0
- Temporary WebP frames and PNG frames are automatically cleaned up via `tempfile::TempDir`

### Testing
- Verified 3-frame WebP (100ms/frame) converts to:
  - GIF: 3 frames, 0.3s duration, 10fps ✅
  - MOV: 3 frames, 0.3s duration, 10fps, HEVC codec ✅
- No frame duplication or timing errors

## [0.10.6] - 2026-03-09

### Fixed
- **AVIF GBR colorspace bug**: Fixed critical bug where AVIF files with GBR colorspace caused HEVC conversion to fail
  - **Root cause**: FFmpeg error "Error setting option colorspace to value gbr" - HEVC doesn't support RGB/GBR colorspace
  - **Fix**: Skip RGB/GBR colorspace parameters in FFmpeg commands; conversion to YUV420p happens in filter chain
  - **Impact**: AVIF files with GBR colorspace can now be converted to HEVC video formats
  - **Files modified**: `shared_utils/src/video_explorer/gpu_coarse_search.rs`, `vid_hevc/src/conversion_api.rs`

- **WebP dimension detection**: Fixed bug where animated WebP files showed 0x0 dimensions
  - **Root cause**: FFmpeg's ffprobe returns 0x0 for animated WebP files
  - **Fix**: Added fallback to image crate and ImageMagick when ffprobe returns 0x0
  - **Impact**: Animated WebP files no longer fail with "Resolution too small" error
  - **File modified**: `shared_utils/src/video_explorer/precheck.rs`

- **WebP decoder reliability**: Added workaround for FFmpeg's unreliable WebP decoder
  - **Root cause**: FFmpeg's WebP decoder fails with "Invalid data found when processing input" for some animated WebP files
  - **Fix**: Pre-convert WebP → APNG using FFmpeg (primary) or ImageMagick (fallback) before processing
  - **Method**: FFmpeg creates APNG with proper frame rate and duration metadata
  - **Impact**: Animated WebP files can now be reliably converted to GIF or HEVC video formats
  - **Files modified**: `vid_hevc/src/animated_image.rs` (both `convert_to_hevc_mp4` and `convert_to_hevc_mp4_matched`)

- **APNG duration detection**: Fixed bug where ImageMagick-created APNG files had no duration metadata
  - **Root cause**: ImageMagick doesn't preserve timing information when converting to APNG
  - **Fix**: Use FFmpeg as primary method for WebP → APNG conversion (preserves frame rate), with ImageMagick as fallback
  - **Impact**: WebP → MOV/MP4 conversion now works correctly with proper duration

### Added
- **Force video mode**: Added `--force-video` flag and `MODERN_FORMAT_BOOST_FORCE_VIDEO` environment variable
  - Skips meme-score check and forces all animated images to be converted to video (MOV/MP4)
  - Useful for advanced users who want consistent video output regardless of meme-score
  - Environment variable approach allows integration with external scripts

### Technical Details
- RGB/GBR colorspace is now filtered out in `build_color_args_from_probe()` and color metadata building
- WebP pre-processing uses FFmpeg (primary) to convert to APNG with proper timing metadata
- ImageMagick is used as fallback if FFmpeg APNG encoding fails
- Temporary APNG files are automatically cleaned up after processing
- Dimension fallback chain: ffprobe → image crate → ImageMagick

### Testing
- Verified AVIF GBR → MOV conversion (no colorspace errors)
- Verified WebP → MOV conversion (proper duration: 0.36s for 3 frames)
- Verified WebP → GIF conversion (successful)
- All test formats (WebP, AVIF GBR, AVIF YUV, GIF) convert successfully

## [0.10.5] - 2026-03-09

### Fixed
- **Animated JXL support**: Fixed critical bug where animated JXL files could not be processed
  - **Root cause**: FFmpeg's `jpegxl_anim` decoder is incomplete and cannot properly decode animated JXL
  - **Fix**: 
    - Added automatic JXL → APNG pre-conversion using `djxl` before FFmpeg processing
    - Duration detection now works for animated JXL (converts to APNG, counts frames)
    - Both GIF and MOV/MP4 conversion routes now support animated JXL
  - **Impact**: Animated JXL files can now be converted to GIF or HEVC video formats
  - **Requirement**: `djxl` tool must be installed (part of libjxl package)

- **Static JXL detection**: Fixed bug where static JXL images were incorrectly identified as animated
  - **Root cause**: FFmpeg reports all JXL files as `jpegxl_anim` codec, even static ones
  - **Fix**: Modified `is_jxl_animated_via_ffprobe()` to convert to APNG and count frames
  - **Impact**: Static JXL images are now correctly skipped (already optimal format)

### Added
- **Static JXL skip logic**: Static JXL images are now explicitly skipped in img-hevc
  - Prevents unnecessary re-encoding of already optimal format
  - Original files are copied to output directory to ensure no data loss
  - Clear messaging: "Source is static JPEG XL (already optimal)"

### Technical Details
- Modified `convert_to_gif_apple_compat()` and `convert_to_hevc_mp4()` to detect JXL format
- Added `try_jxl_via_apng()` function for duration detection via temporary APNG conversion
- Modified `is_jxl_animated_via_ffprobe()` to use djxl+ffprobe for accurate animation detection
- JXL files are automatically converted to APNG intermediate format before FFmpeg processing
- Temporary APNG files are automatically cleaned up after processing

## [0.10.4] - 2026-03-09

### Changed
- **Unified GIF conversion pipeline**: Removed ImageMagick fallback, now all formats use FFmpeg high-quality single-pass method
  - **Rationale**: Quality testing showed ImageMagick and FFmpeg both achieve 256 colors; FFmpeg is simpler and supports multi-stream files
  - **Method**: Single-pass `split+palettegen(256)+paletteuse(bayer)` for all animated formats (AVIF/WebP/JXL/HEIC/etc)
  - **Impact**: Consistent quality across all formats, simplified codebase, better multi-stream support

### Removed
- **ImageMagick dependency**: Completely removed ImageMagick fallback for GIF conversion
  - **Reason**: No quality advantage over FFmpeg, adds complexity, doesn't support multi-stream files
  - **Fallback behavior**: If FFmpeg fails, copy original file and mark as failed (no silent quality degradation)

### Technical Debt Cleanup
- Removed unnecessary ImageMagick code paths
- Simplified GIF conversion logic to single high-quality method
- All formats now use consistent color preservation approach

## [0.10.3] - 2026-03-09

### Fixed
- **Multi-stream animated files frame loss**: Fixed critical bug where multi-stream animated files (AVIF, HEIC, WebP) would only convert the first frame instead of all frames
  - **Root cause**: Files with multiple video streams (thumbnail + animation) defaulted to first stream (1 frame)
  - **Fix**: 
    - `probe_video` now selects stream with most frames
    - Added `stream_index` field to track correct stream
    - FFmpeg uses `-map 0:N` to select animation stream
    - Multi-stream detection skips ImageMagick (doesn't support stream selection)
  - **Impact**: All frames preserved in multi-stream animated files

- **Frame rate preservation**: Removed `-r` parameter that was forcing output frame rate
  - **Issue**: Previous fix incorrectly added `-r` flag which changed original frame rate
  - **Fix**: FFmpeg automatically preserves original frame rate without explicit parameter
  - **Impact**: Original frame rate maintained (e.g., 0.5 fps → 0.5 fps)

### Improved
- **GIF conversion quality**: Upgraded to single-pass high-quality palette method
  - **Old method**: Two-pass with separate palette file (lower quality)
  - **New method**: Single-pass `split+palettegen+paletteuse` (reference: animate-avif best practices)
  - **Impact**: Better color preservation, no temporary palette files

- **Multi-stream handling**: Enhanced detection and processing
  - Automatic multi-stream detection via ffprobe
  - ImageMagick fallback only for single-stream files
  - FFmpeg `-filter_complex [0:N]...` for multi-stream GIF conversion

### Dependencies
- **Updated to GitHub stable versions**: anyhow, thiserror, clap, walkdir, filetime, xattr, which, log, chrono, image, libheif-rs, tempfile, proptest, flate2
- **Kept crates.io**: serde/serde_json (version coupling), rayon (dependency tree), tracing (feature complexity), indicatif/console (tag mismatch)

## [0.10.2] - 2026-03-09

### Fixed
- **Animated AVIF/WebP to MOV conversion frame loss**: Fixed critical bug where animated images (AVIF, WebP, HEIC) converted to HEVC MOV/MP4 would only contain 1 frame instead of all frames. FFmpeg now explicitly receives `-r <fps>` parameter to preserve all frames during conversion.
  - **Root cause**: FFmpeg defaulted to extracting only the first frame when no frame rate was specified for animated image inputs.
  - **Fix**: Added frame rate probing before conversion and explicit `-r` flag in FFmpeg command.
  - **Impact**: Animated images now convert correctly with all frames preserved.

### Improved
- **Meme-score system enhancements**: Improved GIF meme detection algorithm for more reliable identification of memes/stickers vs video clips:
  - **Tightened confidence intervals**: Reduced gray zone from 0.35-0.65 to 0.40-0.60 for more decisive classification
  - **Increased sharpness weight**: Boosted from 0.40 to 0.45 to better detect simple-palette memes
  - **Adjusted dimension weights**: Rebalanced resolution (0.18), duration (0.20), aspect ratio (0.12), and fps (0.05) for better meme detection
  - **Result**: More accurate meme identification while maintaining conservative defaults

### Documentation
- **Meme-score algorithm**: Updated documentation to reflect new confidence thresholds and weight distribution

## [0.10.1] - 2026-03-09

### Fixed
- **FFmpeg libx265 error for animated image containers**: Fixed "Not yet implemented in FFmpeg, patches welcome" error when processing animated AVIF/HEIC/GIF/WebP files. Image containers now use `-map 0:v` (video only) and `-an` (no audio) flags instead of `-map 0` (all streams).
  - **Root cause**: FFmpeg's libx265 encoder failed when trying to map non-existent audio streams from image containers.
  - **Fix**: Added `is_image_container()` detection function and conditional stream mapping in `gpu_coarse_search.rs`.
  - **Impact**: Animated image containers now convert successfully to HEVC without crashes.

- **Audio demux from image containers in x265 mux**: Fixed x265 encoder attempting to demux audio from image containers (AVIF/HEIC/GIF/WebP) during the mux step, causing unnecessary warnings and potential failures.

- **Temporary file cleanup**: Improved cleanup of temporary files during video processing to prevent disk space issues.

- **FPS precheck accuracy**: Enhanced frame rate detection accuracy in precheck phase.

- **Resolution correction**: Fixed resolution detection and correction in video processing pipeline.

- **Precheck warning level**: Downgraded NotRecommended precheck messages from `warn` to `info` level to reduce log noise for expected cases.

### Changed
- **Image container handling**: Image formats (AVIF/HEIC/GIF/WebP/PNG/JPG/JPEG/BMP/TIFF) now have explicit audio-free processing path in FFmpeg commands.
- **FFmpeg command generation**: Improved logic to distinguish between image containers and video files for more appropriate encoding parameters.

### Code Quality
- **Clippy warnings**: Resolved all clippy warnings for improved code quality and maintainability.

### Documentation
- **MIT License**: Added MIT license file to the repository.
- **Third-party licenses**: Added comprehensive third-party license information and acknowledgements.
- **Acknowledgements cleanup**: Removed incorrect Czkawka acknowledgements.

### Dependencies
- **Dependency updates**: Updated all dependencies to latest versions, including incompatible version upgrades where necessary.

## [0.9.9-3] - 2026-03-05

### Apple Compatibility Enhancements

#### Improved Variable Frame Rate (VFR) detection for iPhone slow-motion videos
- **Enhanced VFR detection algorithm**: iPhone slow-motion videos use variable frame rate (VFR) to achieve the slow-motion effect. Without proper handling, ffmpeg converts VFR to constant frame rate (CFR), losing the slow-motion timing.
  - **Increased threshold from 1% to 2%**: Reduces false positives from minor frame rate variations in standard CFR videos.
  - **Apple slow-motion detection**: Checks for `com.apple.quicktime.fullframerate` tag (Apple's private metadata for slow-mo videos) - the most reliable indicator.
  - **Frame rate ratio analysis**: For MOV/MP4 with avg_frame_rate ≥ 60fps, detects slow-mo when r_frame_rate / avg_frame_rate > 2 (recording rate significantly higher than playback rate).
  - **Removed unreliable indicators**: Eliminated checks for deprecated `codec_time_base`, generic `timecode` tags, and `start_time` which are common in normal CFR videos.
  - **Preservation**: When VFR is detected, video conversion automatically adds `-vsync vfr` to ffmpeg arguments, preserving the variable frame rate in the output.
  - **Impact**: Significantly reduced false positives while accurately detecting actual VFR content including iPhone slow-motion recordings.

#### AAE file handling for Apple Photos editing metadata
- **Added AAE file detection and handling**: AAE (Apple Adjustment Envelope) files store photo editing metadata from iPhone/Photos.app. When source images are converted to modern formats, AAE files become orphaned and lose their association.
  - **Function**: Added `handle_aae_file()` in `shared_utils/src/conversion.rs` to detect and handle AAE files (case-insensitive .aae/.AAE).
  - **Apple Compat mode**: AAE files are migrated to the output directory alongside converted images, preserving editing metadata.
  - **Non-compat mode**: Orphaned AAE files are deleted to avoid clutter.
  - **Impact**: Photo editing metadata is preserved in Apple Compat workflows, preventing loss of editing history.

## [Unreleased]

## [0.9.9-2] - 2026-03-05

### Changes

#### GIF conversion: ImageMagick-first strategy
- **GIF encoding now tries ImageMagick first**, then falls back to ffmpeg two-pass palette. This eliminates the "⚠️ ffmpeg GIF encode failed" log noise and correctly handles animated WebP (ANIM/ANMF) which ffmpeg 8.x cannot decode.

#### Fail-safe: all animated conversion failures copy original file
- **`convert_to_hevc_mp4`**: ffmpeg encode failure or invalid output → copy original instead of returning `Err`.
- **`convert_to_hevc_mkv_lossless`**: same fail-safe applied.
- **`convert_to_hevc_mp4_matched`**: `quality_or_compat_ok=false` path now calls `mark_as_processed` to avoid re-processing.
- **`convert_to_gif_apple_compat`**: both-encoders-failed path copies original. Invalid output (empty/unreadable) also copies original instead of returning `Err`.
- No conversion failure can result in a missing output file — data is always preserved.

## [0.9.9-1] - 2026-03-05

### Bug Fixes

#### Animated WebP→GIF: ffmpeg fallback to ImageMagick
- **Fixed animated WebP producing no output in apple_compat GIF route**: ffmpeg 8.x does not support animated WebP (ANIM/ANMF chunks) — palette generation silently failed, causing the second ffmpeg pass to error on a missing palette file, and the entire conversion to propagate an error with no output file.
  - **Root cause**: `convert_to_gif_apple_compat()` in `vid_hevc/src/animated_image.rs` only used ffmpeg two-pass palette approach with no fallback for formats ffmpeg cannot decode.
  - **Fix**: When ffmpeg palette generation fails or the palette file is not created, fall back to `magick`/`convert` (ImageMagick) with `-coalesce -layers optimize`. ImageMagick handles animated WebP correctly.
  - **Impact**: Animated WebP files in apple_compat mode now correctly produce GIF output instead of erroring out silently.

#### Animated routing: unified meme-score strategy
- **Removed hardcoded 4.5s duration threshold** from apple_compat animated routing. The old logic used `duration >= 4.5s || resolution >= 720p` to decide HEVC vs GIF. Both apple_compat and non-compat branches now use the meme-score multi-dimensional heuristic (duration, resolution, fps, aspect, bytes/pixel) for consistent decisions.
- **Removed redundant internal short-animation skip** in `convert_to_hevc_mp4_matched()` and `convert_to_gif_apple_compat()` — these were double-checking duration after meme-score already made the decision, and were harmful in apple_compat mode (would copy non-playable originals).

## [0.9.9] - 2026-03-05

### Bug Fixes

#### Animated Modern Format Detection — Comprehensive Fix
- **Fixed animated AVIF passthrough bug**: Animated AVIF files (ISOBMFF major_brand `avis` or compatible_brand `msf1`) were incorrectly treated as static images, causing them to be copied to the output directory unchanged instead of being routed through the Apple Compat conversion pipeline (HEVC MP4 / GIF).
  - **Root cause (2 layers)**:
    1. `detect_animation()` in `image_detection.rs` had no AVIF branch — the `_ => Ok((false, 1, None))` fallback silently returned non-animated.
    2. `analyze_avif_image()` in `image_analyzer.rs` hardcoded `is_animated: false`, so even if detection were fixed, the analysis result would still report static.
  - **Fix**: Added `DetectedFormat::AVIF` branch to `detect_animation()` using the new `is_isobmff_animated_sequence()` helper (reads ftyp box major_brand + compatible_brands for `avis`/`msf1`). Updated `analyze_avif_image()` to call `detect_animation()` and set `is_animated`/`duration_secs` correctly.
  - **Impact**: Animated AVIF in Apple Compat mode now correctly routes to HEVC MP4 (long/high-res) or GIF (short/low-res) instead of being silently passed through.

- **Fixed animated JXL never detected**: `analyze_jxl_image()` hardcoded `is_animated: false` and `detect_animation()` had no JXL branch.
  - **Fix**: Added `DetectedFormat::JXL` branch to `detect_animation()` using `is_jxl_animated_via_ffprobe()` (checks ffprobe duration > 0, falls back to jxlinfo "animation" keyword). Updated `analyze_jxl_image()` to call `detect_animation()`.
  - **Impact**: Animated JXL files now correctly enter the animated conversion pipeline instead of being treated as static JXL (which would skip them entirely as "already optimal").

- **Fixed HEIC/HEIF animation metadata always false**: `analyze_heic_image()` hardcoded `is_animated: false`. While this doesn't affect routing (HEIC/HEIF are intercepted by `is_apple_native` guard), it caused incorrect metadata in analysis results.
  - **Fix**: Added `is_isobmff_animated_sequence()` call to set correct `is_animated` and `duration_secs`.
  - **Impact**: Metadata correctness for downstream consumers; no routing behavior change.

- Affected tools: **img-hevc**, **img-av1** (both share `shared_utils` analysis layer)

#### Deep Audit Fixes
- **Fixed `make_routing_decision()` ignoring `is_animated` parameter**: The `_is_animated` parameter was unused (prefixed underscore), causing animated modern lossy formats (AVIF/JXL/HEIC/HEIF) to return `should_skip: true` even when animated. Now correctly allows animated modern formats to pass through to the animated conversion pipeline.
  - **File**: `shared_utils/src/image_quality_detector.rs`

- **Fixed img_av1 `copy_on_skip_or_fail` error swallowing**: Two paths in `img_av1/src/conversion_api.rs` (NoConversion skip + compress-mode rejection) used `let _ =` to discard copy errors, silently losing files. Now properly propagates errors. (img_hevc was already fixed in v0.9.8.)

- **Fixed JXL distance format precision loss in fallback path**: `img_hevc/src/lossless_converter.rs` FFmpeg→cjxl fallback pipeline used `{:.1}` (1 decimal) for distance while the primary path used `{:.2}` (2 decimals), causing precision loss (e.g. `d=0.85` → `d=0.9`). Now consistent `{:.2}` everywhere.

- **Fixed `--lossless_jpeg=0` applied to non-JPEG inputs**: `convert_to_jxl_matched()` in both img_hevc and img_av1 unconditionally passed `--lossless_jpeg=0` when `distance > 0`, even for PNG/WebP/TIFF inputs. Now only applied when `input_format` is JPEG.

#### Apple Compat Size/Quality Guard Bypass
- **Fixed apple_compat mode copying non-playable original on size guard trigger**: In `vid_hevc/src/animated_image.rs`, the `convert_to_hevc_mp4_matched()` size guard (output > input) would fall back to copying the original file in apple_compat mode. However, the original (e.g. animated AVIF) is not playable on Apple devices. A larger HEVC file is always preferable to a non-playable original.
  - **Fix**: Added `size_guard_active = !options.apple_compat` so the size guard is bypassed entirely in apple_compat mode.
- **Fixed quality check gate blocking apple_compat HEVC output**: A second guard (`quality_passed=false` when video stream couldn't be compressed below input size) was also discarding the HEVC file and copying the original. Same apple_compat override applied.
  - **Fix**: Added `quality_or_compat_ok = quality_passed || (apple_compat && SSIM ≥ 0.90)` to allow high-quality HEVC output regardless of file size when in apple_compat mode.
- **Fixed same size guard in `convert_to_gif_apple_compat()`**: GIF path had an identical size guard that would copy non-playable original; same fix applied.
- **Impact**: Animated AVIF (and other non-Apple-native animated formats) in apple_compat mode now always produce a playable HEVC MP4 or GIF output, even if larger than the original.

## [0.9.8] - 2026-03-04

### Bug Fixes

#### Linux ACL Preservation
- **Fixed `dst` parameter never used bug**: The `preserve_linux_attributes()` function previously used `setfacl --restore=-` which restored ACL to the **source file itself**, completely ignoring the `dst` parameter.
  - **Root cause**: Piped `setfacl --restore=-` reads ACL from stdin but applies to the file specified, which was missing
  - **Fix**: Parse ACL output and apply each entry individually using `setfacl -m <entry> <dst>`
  - **Impact**: Linux file permissions and ACLs now correctly propagate to converted output files

#### Error Propagation
- **Propagate `copy_on_skip_or_fail` errors**: Multiple conversion paths previously swallowed errors with `let _ =`:
  - `img_hevc/src/conversion_api.rs`: 2 skip/compress paths
  - `vid_hevc/src/conversion_api.rs`: 6 paths (5 skip/compress + 1 temp commit)
  - **Behavior change**: Failures now throw `ImgQualityError::ConversionError` or `VidQualityError::GeneralError` instead of silently returning success
  - **Impact**: Conversion failures are now properly reported to users instead of fabricating successful results

- **Propagate `commit_temp_to_output` errors**: Apple compatibility fallback path in `vid_hevc` now propagates temp-to-output commit failures with `?` instead of `let _ =`

#### Apple Photos Library Protection
- **Added Apple Photos library detection**: Prevents direct file manipulation inside `.photoslibrary` / `.photolibrary` packages
  - Checks at entry points before any processing (img_hevc, img_av1, vid_hevc, vid_av1)
  - Clear error message with guidance to export photos first
  - Includes unit tests for detection logic
  - **Impact**: Prevents accidental corruption of Photos database and data loss

---

### Code Quality
- **Removed fabricated `ExitStatus::default()` in fallback pipelines**: The FFmpeg→cjxl and ImageMagick→cjxl fallback pipelines previously constructed a fake `std::process::Output { status: ExitStatus::default() }` to signal success — semantically incorrect and fragile. Refactored all fallback paths to early-return with proper `ConversionResult` via `finalize_with_size_check` / `finalize_fallback_jxl`, eliminating fake process output entirely.
  - Affected files: `img_hevc/src/lossless_converter.rs`, `img_av1/src/lossless_converter.rs`, `shared_utils/src/jxl_utils.rs`
  - `run_imagemagick_cjxl_pipeline` now returns `Result<(), ...>` instead of `Result<Output, ...>`
  - `try_imagemagick_fallback` now returns `io::Result<()>` instead of `io::Result<Output>`

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


--- ARCHIVAL COMMIT HISTORY (Full Ledger) ---


* 2025-12-10 | ba77a9d | chore: add project files
* 2025-12-11 | d2b35a2 | feat: video tools default to --match-quality enabled, image tools default to disabled
* 2025-12-11 | 870bf01 | feat: unified quality_matcher module for all tools
* 2025-12-11 | 4982b00 | fix: match_quality only for lossy sources, lossless uses CRF 0
* 2025-12-11 | d0a23fe | feat: enhanced quality_matcher with cutting-edge codec support
* 2025-12-11 | 0f8293b | refactor: modularize skip logic with VVC/AV2 support
* 2025-12-11 | b729a4c | fix: remove silent fallbacks in quality_matcher (Quality Standard)
* 2025-12-11 | 77f068f | 🔥 Quality Matcher v3.0 - Data-Driven Precision
* 2025-12-11 | 91f6f57 | 🔬 Add strict precision tests and edge case validation
* 2025-12-11 | 934fb4d | 🔬 Image Quality Detector - Precision-Validated Auto Routing
* 2025-12-11 | a9ed8c9 | feat(shared_utils): add video_quality_detector module with 56 precision tests
* 2025-12-11 | fb5fd7a | feat(shared_utils): expand precision tests for ffprobe and conversion modules
* 2025-12-11 | 1e59703 | feat(shared_utils): add comprehensive codec detection tests
* 2025-12-11 | e1d3e61 | feat(shared_utils): add batch/report precision tests and README
* 2025-12-11 | 93fd0d8 | feat(video_explorer): 模块化探索功能 + 精确度规范
* 2025-12-11 | 497abd5 | feat(imgquality-hevc): add --explore flag for animated→video conversion
* 2025-12-11 | 311063f | feat(shared_utils): enhance precision validation and SSIM/PSNR calculation
* 2025-12-11 | e81e4e6 | fix(video_explorer): add scale filter for SSIM/PSNR calculation
* 2025-12-11 | c6be1fc | feat(video_explorer): add VMAF support for quality validation v3.3
* 2025-12-11 | f52e1f8 | v3.5: Enhanced quality matching with full field support
* 2025-12-11 | d728727 | v3.6: Enhanced PNG lossy detection via IHDR chunk analysis
* 2025-12-11 | 5e81555 | 🔥 v3.7: Enhanced PNG Quantization Detection with Referee System
* 2025-12-11 | 34b1ba2 | 🔧 Code Quality Improvements
* 2025-12-11 | 15a2a03 | feat: Complete drag & drop one-click processing system
* 2025-12-11 | f3072d9 | fix: vidquality-hevc --match-quality requires explicit value
* 2025-12-11 | 069dee7 | fix: 🛡️ Protect original files when quality validation fails (CRITICAL)
* 2025-12-11 | bdf3beb | refactor: Code quality improvements + README update (v3.8)
* 2025-12-11 | a1648f7 | perf: Code quality improvements and clippy fixes
* 2025-12-11 | c833a4c | fix: Remove all clippy warnings
* 2025-12-11 | 5724f23 | feat: Add XMP metadata merge before format conversion v3.9
* 2025-12-11 | a3fccfc | cleanup: Remove accidentally committed test file
* 2025-12-11 | 7ca2d6d | fix: resolve clippy warnings and type errors
* 2025-12-11 | 163d6d1 | refactor: implement real functionality, remove TODO placeholders
* 2025-12-11 | aefdf1c | fix: resolve remaining clippy warnings in imgquality_API
* 2025-12-11 | 40e9b0a | refactor: introduce AutoConvertConfig struct to fix too_many_arguments warning
* 2025-12-12 | dfe8438 | fix: XMP 合并时保留媒体文件的原始时间戳
* 2025-12-12 | 2416812 | fix: 修复 metadata/timestamps 保留顺序问题
* 2025-12-12 | b12d126 | 🍎 苹果兼容模式裁判测试完善 + H.264 精度验证 + 编译警告修复
* 2025-12-12 | b56429b | feat: 断点续传 + 原子操作保护
* 2025-12-12 | 555c18e | feat: 新增测试模式 v4.2
* 2025-12-12 | a3451cb | fix: 测试模式修复 + 增强边缘案例采样
* 2025-12-12 | a8a751b | fix: 修复测试模式采样问题
* 2025-12-12 | 2b09971 | feat: 🍎 Apple 兼容模式增强 - 现代动态图片智能转换
* 2025-12-12 | 5335a3a | refactor: rename vidquality_API → vidquality_av1, imgquality_API → imgquality_av1
* 2025-12-12 | a77a90f | feat(test-mode): v4.3 随机采样 + 多样性覆盖
* 2025-12-12 | f8afdf7 | fix: 使用 Homebrew bash 5.x 支持 local -n 特性
* 2025-12-12 | 9728979 | chore: 使用 Homebrew bash 5.x 替代系统 bash 3.x
* 2025-12-12 | 78bffd6 | feat: 新增 XMP Merger Rust 模块 - 可靠的元数据合并
* 2025-12-12 | b7f4554 | feat: XMP Merger v2.0 - 增强可靠性
* 2025-12-12 | 2af2a2d | feat: Expand XMP merger file type support and matching strategies
* 2025-12-12 | 3586534 | fix: Add .jpe, .jfif, .jif JPEG variants to supported extensions
* 2025-12-12 | 00f6142 | refactor: switch XMP merger from whitelist to blacklist approach
* 2025-12-12 | e8b67c4 | fix: always restore original media timestamp after XMP merge
* 2025-12-12 | 4a8152d | feat: add checkpoint/resume support to XMP merger
* 2025-12-12 | 897232f | fix: improve lock file detection to avoid false positives
* 2025-12-12 | 3f7a213 | fix: add WebP fallback for cjxl 'Getting pixel data failed' error
* 2025-12-12 | 28a1d26 | refactor: proactive input preprocessing for cjxl instead of fallback
* 2025-12-12 | 918ee33 | refactor: simplify drag_and_drop_processor v5.0
* 2025-12-12 | 4c4346a | fix: correct CLI argument from --output-dir to --output
* 2025-12-12 | 1aa9dcb | fix: add ImageMagick fallback for cjxl 'Getting pixel data failed' errors
* 2025-12-12 | 27d32ee | enhance: add comprehensive transparency for fallback mechanisms
* 2025-12-12 | 0884fc0 | 修复视频处理中'Output exists'被错误计为失败的问题
* 2025-12-12 | 1f6316e | 🔥 根源修复：Output exists 返回跳过状态而非错误
* 2025-12-12 | b16ae55 | 🔬 v3.5: 增强裁判机制 (Referee Mechanism Enhancement)
* 2025-12-12 | 4703967 | 🎯 v3.6: 三阶段高精度搜索算法 (±0.5 CRF)
* 2025-12-12 | 6dc3ca5 | v3.7: Dynamic threshold adjustment for low-quality sources
* 2025-12-13 | 61bdf5b | v3.8: Intelligent threshold system - eliminate hardcoding
* 2025-12-13 | cf5f5b6 | v3.9: Fix --explore --match-quality to MATCH source quality, not minimize size
* 2025-12-13 | 175f44b | v4.0: 激进精度追求 - 无限逼近 SSIM=1.0
* 2025-12-13 | 2b1a626 | v4.1: 三重交叉验证 + 完整透明度
* 2025-12-13 | c3ca9f3 | v4.2: 实时日志输出 - 解决长时间编码终端冻结问题
* 2025-12-13 | 795e319 | v4.3: 优化搜索策略 - 大幅减少无意义迭代
* 2025-12-13 | 53beb43 | v4.4: 智能质量匹配 - 根本性设计改进
* 2025-12-13 | 168ef3c | v4.4: 修正术语 - 移除误导性的 AI 描述
* 2025-12-13 | 06339f4 | v4.5: 精确质量匹配 - 恢复正确语义 + 高效搜索
* 2025-12-13 | 121a4b8 | v4.5: 新增 --compress flag - 精确质量匹配 + 压缩
* 2025-12-13 | 2da7915 | v4.5: 添加单元测试 + 实际测试验证
* 2025-12-13 | a32b126 | 🔥 v4.6: Flag 组合模块化 + 编译警告修复
* 2025-12-13 | dcb2ed1 | 🔥 v4.6: 精度提升到 ±0.1 + 算法深度复盘文档
* 2025-12-13 | 91819a7 | 🔥 v4.7: Bug 修复 + 术语澄清
* 2025-12-13 | e3862a1 | 🔥 v4.8: 性能优化 + 缓存机制
* 2025-12-13 | 18ce9c3 | 🔥 v4.8: 性能优化 + CPU flag + README 更新
* 2025-12-13 | 6c73fd3 | 🔧 v4.8: 代码统一 - 消除重复实现
* 2025-12-13 | 9cac2d4 | v4.12: Add 0.1 fine-tune phase to explore_precise_quality_match_with_compression
* 2025-12-13 | 768b5b0 | v4.12: Bidirectional 0.1 fine-tune search
* 2025-12-13 | 387ef8c | v4.13: Smart early termination with variance & change rate detection
* 2025-12-13 | 4efdb57 | v4.13: Fix doc test + Update README (EN/CN)
* 2025-12-13 | 118ddaa | v5.1: Improve UX + Add v4.13 tests
* 2025-12-13 | e875faf | v5.1: Fix GIF conversion + Real animated media tests
* 2025-12-13 | cb1bc06 | v5.1: Verified animated image → video conversion
* 2025-12-13 | b396725 | 🔥 v5.0: 智能 GPU 控制 + 自动 fallback
* 2025-12-13 | 96a2372 | 🐛 修复：min_crf 能压缩时跳过精细调整阶段的问题
* 2025-12-13 | cd08512 | 🐛 修复：Phase 3 必须用 CPU 重新编码最终结果
* 2025-12-13 | 4429e87 | 🔥 v5.1: GPU 粗略搜索 + CPU 精细搜索智能化处理
* 2025-12-13 | aa067df | v5.1.1: 响亮报告 GPU 粗略搜索和 Fallback - GPU 粗略搜索阶段明确显示 --cpu flag 被忽略 - Fallback 情况都有醒目的框框提示
* 2025-12-13 | 664934d | v5.1.2: 从双击 app 脚本中移除 --cpu flag - 移除 drag_and_drop_processor.sh 中的 --cpu flag - 撤回之前的忽略 --cpu flag 报告（没有意义） - 保留 Fallback 响亮报告
* 2025-12-13 | ccd0145 | v5.1.3: 修复 - 实际调用新的 GPU+CPU 智能探索函数 - vidquality_hevc 和 imgquality_hevc 的 PreciseQualityWithCompress 模式现在使用 explore_hevc_with_gpu_coarse - 之前的代码仍然调用旧的 explore_precise_quality_match_with_compression_gpu
* 2025-12-13 | 855f26c | v5.1.4: 修复 GPU 粗略搜索性能和日志重复问题
* 2025-12-13 | 1dbabf1 | 🔥 v5.2: Fix Stage naming + Add 0.1 fine-tuning when min_crf compresses
* 2025-12-13 | 5a508fb | 🔥 v5.2: Fix GPU range design - GPU only narrows upper bound, not lower
* 2025-12-13 | 90725a6 | 🔥 v5.2: Fix Stage B upward search - update best_boundary when finding lower CRF
* 2025-12-14 | 71aeaa0 | Fix GPU/CPU CRF mapping display
* 2025-12-14 | a73e808 | v5.3: Improve GPU+CPU search accuracy
* 2025-12-14 | 2da6b7d | v5.3: Smart short video handling + README update
* 2025-12-14 | 20408ff | v5.3: Extract hardcoded values to constants + Simplify README
* 2025-12-14 | 955b37d | v5.4: GPU three-stage fine-tuning + CPU upward search
* 2025-12-14 | 3da73aa | v5.5: Fix VideoToolbox q:v mapping (1=lowest, 100=highest)
* 2025-12-14 | 83720d3 | v5.6: GPU SSIM validation + dual fine-tuning
* 2025-12-14 | f1b00b4 | v5.6.1: Extract GPU iteration limits to constants + README update
* 2025-12-14 | 7828422 | v5.7: Extend GPU CRF range for higher quality search
* 2025-12-14 | bc788f8 | 🔥 v5.18: Add cache warmup optimization + fix v5.17 performance protection integration
* 2025-12-14 | 5d30664 | 🐛 Fix: --explore --compress now correctly reports error
* 2025-12-14 | 6e8bae0 | 🎨 v5.19: Add modern UI/UX module
* 2025-12-14 | 67731fe | 🔥 v5.20: Add RealtimeExploreProgress with background thread
* 2025-12-14 | 70724cf | 🔥 v5.21: Fix early termination threshold + real bar progress
* 2025-12-14 | 3eaf05c | v5.25: Progress bar + exploration improvements
* 2025-12-14 | 1d3de30 | 🚀 v5.33: 设计效率优化 + 进度条稳定性改进
* 2025-12-14 | 5011cba | 🚀 v5.34: 进度条重构 - 基于迭代计数（GPU部分已修复）
* 2025-12-14 | 5e2aceb | 🔥 v5.34: 完全重构进度条系统 - 从CRF映射→迭代计数
* 2025-12-14 | 0cd30d6 | 🔥 v5.35: 修复进度条冻结 - 禁用GPU并行探测阻塞
* 2025-12-14 | dda3638 | 🔥 v5.35: 防止键盘干扰 - 禁用终端echo
* 2025-12-14 | 39d4c0f | 🔥 v5.35: 脚本强制重新编译 - 确保使用最新代码修复
* 2025-12-14 | 4943392 | 🔥 v5.35: 改进终端控制 - 禁用icanon和输入缓冲
* 2025-12-14 | 33392f5 | 🔥 v5.35: 三重修复 - 解决进度条冻结+终端崩溃+慢速编码
* 2025-12-14 | 081c214 | 🔥 v5.35: 最终方案 - 在shell层面禁止键盘输入
* 2025-12-14 | 8119b8f | 🔥 v5.35: 防止刷屏 - 静默模式禁用GPU搜索详细日志
* 2025-12-14 | e8efcea | 🔥 v5.35: 彻底简化进度显示 - 移除旧进度条混乱
* 2025-12-14 | c025ca5 | 🔥 v5.35: 最终方案 - 关闭stdin文件描述符
* 2025-12-14 | c0825a9 | 🔥 v5.36: 多层键盘交互防护 - 彻底阻止终端输入干扰
* 2025-12-14 | 7ea7a59 | 🔥 v5.38: 完全修复键盘输入污染 - 实现 + 验证成功
* 2025-12-14 | 34dae4b | 🔥 v5.39: 键盘输入保护 - 移除冻结 hidden() 模式，改用 100Hz 刷新 + 强化终端设置
* 2025-12-14 | d8abf9f | 🔥 v5.40: 修复编译警告 + 改进构建脚本
* 2025-12-14 | e988c8a | 🔥 v5.41: 激进的键盘输入防护 - 多重防线完全禁用终端输入
* 2025-12-14 | 7bf3ff1 | 🔥 v5.42: 完全修复键盘输入污染 - 实时进度更新
* 2025-12-14 | e929be8 | 🔥 v5.43: GPU编码超时保护 + I/O优化 - 完全修复Phase 1挂起
* 2025-12-14 | 7327fad | 🔥 v5.44: 简化超时逻辑 - 仅保留 12 小时底线超时，响亮 Fallback
* 2025-12-14 | aca5365 | 🔥 v5.45: 智能搜索算法 - 收益递减终止 + 压缩率修复
* 2025-12-14 | 30bf7dd | 🔥 v5.46: 修复 GPU 搜索方向 - 使用 initial_crf 作为起点
* 2025-12-14 | 162e0aa | 🔥 v5.47: 完全重写 GPU Stage 1 搜索 - 双向智能边界探测
* 2025-12-14 | 8ecdf4d | 🔥 v5.48: 简化 CPU 搜索 - 仅在 GPU 边界附近微调
* 2025-12-14 | 132b1e4 | 🔥 v5.49: 增加 GPU 采样时长 - 提高映射精度
* 2025-12-14 | 082093b | 🔥 v5.50: GPU 搜索目标改为 SSIM 上限 + 10分钟采样
* 2025-12-14 | 7b674d4 | 🔥 v5.51: 简化 GPU Stage 3 搜索逻辑 - 0.5 步长 + 最多 3 次尝试
* 2025-12-14 | 710757d | 🔥 v5.52: 完整重构 GPU 搜索 - 智能采样 + SSIM+大小组合决策 + 收益递减
* 2025-12-14 | 5074dd1 | 🔥 v5.53: 修复 GPU 迭代限制 + CPU 采样编码
* 2025-12-14 | 72d98fb | 🔥 v5.54: 修复 CPU 采样导致最终输出不完整的严重 BUG
* 2025-12-14 | 2aa6c88 | 📦 v5.54 稳定版本备份 - 准备开始柔和改进
* 2025-12-14 | 6ee65bc | 🔥 v5.55: 恢复三阶段结构 + 智能提前终止
* 2025-12-15 | c57f03c | 🔥 v5.55: CPU 精度调整 0.1 → 0.25（速度提升 2-3 倍）
* 2025-12-15 | 548d52f | v5.56: 添加预检查(BPP分析)和GPU→CPU自适应校准
* 2025-12-15 | 4f33660 | v5.57: 添加置信度评分系统
* 2025-12-15 | 3dff3cd | v5.58: 最终编码实时进度显示
* 2025-12-15 | dc32aee | v5.59: 可压缩空间检测 + 动态精度选择
* 2025-12-15 | fedd7e4 | v5.60: 保守智能跳过策略 - 连续3个CRF大小变化<0.1%才跳过
* 2025-12-15 | 031f264 | v5.60: CPU全片编码策略 - 100%准确度，移除采样误差
* 2025-12-15 | eddaf16 | v5.61: 动态自校准GPU→CPU映射系统 - 通过实测建立精确映射
* 2025-12-15 | 5182b82 | v5.62: 双向验证+压缩保证 - 修复搜索方向，确保最高SSIM且能压缩
* 2025-12-15 | d9b094e | v5.63: 双向验证 + 压缩保证
* 2025-12-15 | b8bfc06 | v5.64: GPU 多段采样策略
* 2025-12-15 | db0c427 | v5.65: GPU 精细搜索后 CPU 窄范围验证
* 2025-12-15 | 5f70d27 | v5.66: GPU 质量天花板概念 + 分层接力策略基础
* 2025-12-15 | 239b356 | v5.67: 边际效益递减算法 + 颜色UI改进
* 2025-12-15 | 2ac555b | v5.67.1: 全面英语化输出日志
* 2025-12-15 | f1f7120 | 🔥 v5.70: Smart Build System - 智能编译系统
* 2025-12-15 | dc49402 | feat(precheck): v5.71 - Fix legacy codec handling and smart FPS detection
* 2025-12-15 | d17a724 | v5.72: Add robustness improvements - LRU cache, unified error handling, three-phase search, detailed progress
* 2025-12-15 | e9960eb | fix(v5.72): Correct GPU+CPU dual refinement strategy
* 2025-12-15 | afb21a8 | v5.74: 备份 - 开始透明度改进 spec
* 2025-12-15 | 116b8f3 | v5.74: 透明度改进 - PSNR→SSIM映射 + Preset一致性 + Mock测试
* 2025-12-15 | f53adb1 | feat(gpu): Implement GPU quality ceiling detection v5.80
* 2025-12-15 | aef11f8 | fix(gpu): Clarify compression boundary vs quality ceiling
* 2025-12-15 | 0133a29 | feat(v5.76): auto-merge XMP sidecar files during conversion
* 2025-12-15 | 1bf0312 | fix(cache): Unify cache key mechanism to prevent cache misses
* 2025-12-15 | e230c25 | feat(progress): Add unified println() method for log output
* 2025-12-15 | c0e5e25 | feat(vmaf): Add VMAF verification for short videos (≤5min)
* 2025-12-15 | a058949 | v5.75: VMAF-SSIM synergy - 探索用SSIM，验证用VMAF
* 2025-12-16 | 0e59949 | v5.81: Adaptive multiplicative CPU search - 67% fewer iterations
* 2025-12-16 | b84fe45 | v5.82: Smart adaptive CPU search with target compression
* 2025-12-16 | 0505723 | v5.83: High quality target - SSIM threshold 0.995
* 2025-12-16 | 8ff02f1 | feat(cpu): CPU步进算法v5.87 - 自适应大步长+边际效益+GPU对比
* 2025-12-16 | 263bbf3 | 🔥 v5.87: VMAF与SSIM协同改进 - 5分钟阈值
* 2025-12-16 | e356146 | 🔥 v5.88: 进度条统一 - DetailedCoarseProgressBar
* 2025-12-16 | f827da6 | 🔥 v5.89: CPU步进算法深入改进 - 递进式步长+过头回退
* 2025-12-16 | 8019089 | 🔥 v5.90: CPU自适应动态步进 - 数学公式驱动（用户建议）
* 2025-12-16 | d2e10c7 | 🔥 v5.91: 强制过头策略 - 必须找到真正边界
* 2025-12-16 | 2f7a6ae | v5.93: 智能撞墙算法 - 质量墙检测
* 2025-12-16 | be4257c | v5.94: Fix VMAF quality grading thresholds + cleanup warnings
* 2025-12-16 | bc4f88a | v5.95: 激进撞墙算法 - 扩大CPU搜索范围(3→15 CRF)
* 2025-12-16 | 701a198 | v5.97: Ultra-aggressive CPU stepping strategy
* 2025-12-16 | 535867f | v5.98: Curve model aggressive stepping - exponential decay (step × 0.4^n), max 4 wall hits, 87.5% iteration reduction
* 2025-12-16 | 5a6f32b | v5.99: Curve model + fine tuning phase - switch to 0.1 step when curve_step < 1.0
* 2025-12-16 | f842c35 | v6.0: GPU curve model strategy - aggressive wall collision + fine backtrack in GPU phase
* 2025-12-16 | 5fb76c2 | v6.1: Boundary fine tuning - auto switch to 0.1 step when reaching min_crf boundary
* 2025-12-16 | 2af40e4 | backup: before Strategy pattern refactoring v6.3
* 2025-12-16 | 4e0f883 | feat(v6.3): Strategy pattern for ExploreMode - SSIM/Progress unified
* 2025-12-16 | 6265b26 | test(v6.3): add property-based tests for Strategy pattern
* 2025-12-16 | 7c9db2e | v6.4.4: Code quality improvements - Strategy helper methods (build_result, binary_search_compress, binary_search_quality, log_final_result) reduce ~40% duplicate code - Enhanced Rustdoc comments with examples for public APIs - SsimResult helpers: is_actual(), is_predicted() methods - Boundary tests for metadata margin edge cases - All 505 tests pass
* 2025-12-16 | 206b765 | v6.4.5: Performance & error handling improvements
* 2025-12-16 | 40eaeb6 | v6.4.6: Technical debt cleanup
* 2025-12-16 | a197427 | v6.5.0: Unified CrfCache refactor - Replace HashMap with CrfCache in gpu_accel.rs
* 2025-12-16 | 333b9ad | v6.6.0: Complete cache unification - All HashMap migrated to CrfCache
* 2025-12-16 | a62821d | spec: code-quality-v6.4.6 requirements and design
* 2025-12-16 | f9c7759 | feat(v6.4.7): 代码质量修复 - CrfCache精度升级/GPU临时文件扩展名/FFmpeg进程管理
* 2025-12-16 | e423454 | feat(v6.4.8): 苹果兼容模式使用 MOV 容器格式
* 2025-12-16 | 0e7733b | Revert "feat(v6.4.8): 苹果兼容模式使用 MOV 容器格式"
* 2025-12-16 | ced5135 | feat(v6.4.8): --apple-compat 模式使用 MOV 容器格式
* 2025-12-16 | 44659b6 | feat(v6.4.8): vidquality_hevc 也支持 --apple-compat MOV 输出
* 2025-12-16 | 3bfa99c | feat(v6.4.9): 代码质量与安全性修复
* 2025-12-16 | 21f71ea | fix: doctest ignore 标记修复
* 2025-12-17 | e49aab9 | v6.5.1: 取消硬上限机制，改为保底机制
* 2025-12-17 | 387506e | fix(v6.6.1): 修复 CPU Fine-Tune 阶段长视频卡死问题
* 2025-12-18 | 2199bea | 🔥 v6.7: Container Overhead Fix - Pure Media Comparison
* 2025-12-18 | eed101b | 🔧 v6.8: Fix FPS parsing - correct ffprobe field order
* 2025-12-18 | 19fd831 | v6.9: Adaptive zero-gains + VP9 duration detection
* 2025-12-18 | 28f7855 | fix: suppress dead_code warnings for serde fields
* 2025-12-18 | 0787f1e | 🔥 v7.0: Fix test quality issues - eliminate self-proving assertions
* 2025-12-18 | 23a0dbd | feat(v7.1): Add type-safe wrappers for CRF, SSIM, FileSize, IterationGuard
* 2025-12-18 | 9334d90 | v7.1.1: Gradual migration to type-safe wrappers
* 2025-12-18 | 9058806 | v7.1.2: Add type-safe helpers to gpu_accel.rs
* 2025-12-18 | eead475 | v7.1.3: Add type-safe helpers to more modules
* 2025-12-18 | c9224d1 | fix(v6.8): CRF超出范围导致编码失败 + dead_code警告
* 2025-12-18 | 042459d | v6.8: Fix evaluation consistency - use pure video stream comparison
* 2025-12-19 | 213c007 | v6.9.1: Smart audio transcoding + cleanup
* 2025-12-19 | 57050de | chore: move smart_build.sh to scripts/, update drag_and_drop path
* 2025-12-20 | a276503 | chore: auto-sync changes
* 2025-12-20 | 76ffa06 | fix: VP8/VP9压缩失败和GPU搜索范围问题
* 2025-12-20 | 6e1ba1b | fix: MS-SSIM功能修复
* 2025-12-20 | 32e0a21 | feat(v6.9): MS-SSIM as target threshold (not just verification)
* 2025-12-20 | 9b1421a | fix(v6.9.1): Clamp MS-SSIM to valid range [0, 1]
* 2025-12-20 | 8817828 | fix(v6.9.2): Fix MS-SSIM JSON parsing - use pooled_metrics mean
* 2025-12-20 | 5062efc | feat(v6.9.3): Add SSIM All comparison and chroma loss detection
* 2025-12-20 | c9f8f67 | feat(v6.9.4): Use SSIM All as final quality threshold (includes chroma)
* 2025-12-20 | a8866db | fix(v6.9.5): Use dynamic SSIM threshold from explore phase in Phase 3
* 2025-12-20 | c7979c2 | feat(v6.9.6): MS-SSIM as primary quality judgment
* 2025-12-20 | 3ed3d44 | refactor(v6.9.6): Use SSIM All exclusively, remove MS-SSIM
* 2025-12-20 | a762879 | feat(v6.9.6): Implement 3-channel MS-SSIM (Y+U+V) for accurate quality verification
* 2025-12-20 | dbf16b8 | feat(v6.9.7): Enhance fallback warnings and add MS-SSIM vs SSIM test
* 2025-12-20 | 1d7a24a | v6.9.8: Fusion quality score (0.6×MS-SSIM + 0.4×SSIM_All)
* 2025-12-20 | 414879b | v6.9.9: Use SSIM All for non-MS-SSIM verification
* 2025-12-25 | e889fc6 | fix(xmp): treat ExifTool [minor] warnings as success for JXL container wrapping
* 2025-12-25 | 3c3947d | fix(imgquality): correct error message when video stream compression fails
* 2025-12-25 | 674486f | fix(xmp): merge XMP sidecars for skipped files
* 2026-01-16 | 6ba3acf | v6.9.12: 格式支持增强 + 验证机制
* 2026-01-16 | 20585b3 | v6.9.13: 无遗漏设计 - 处理全部文件
* 2026-01-16 | 27d80c1 | v6.9.13: 无遗漏设计 - 核心实现移至Rust
* 2026-01-16 | 3404065 | v6.9.14: 无遗漏设计 - 失败文件回退复制
* 2026-01-16 | c72a8cc | v6.9.15: 无遗漏设计 - 不支持文件的XMP处理
* 2026-01-16 | d508a65 | v6.9.16: XMP合并优先策略
* 2026-01-17 | a72b3cd | fix: 添加转换差异分析和修复脚本
* 2026-01-18 | 24bcb98 | 🔥 v6.9.17: Critical CPU Encoding & GPU Fallback Fixes
* 2026-01-18 | 813b20e | 🔥 v7.2: Quality Verification Fix - Standalone VMAF Integration
* 2026-01-18 | c8719fb | 🔧 Fix vmaf model parameter - remove unsupported version flag
* 2026-01-18 | 4a1cb5a | ✅ Final vmaf fix - correct feature parameter format
* 2026-01-18 | ab0faf1 | 📝 Document: vmaf float_ms_ssim includes chroma information
* 2026-01-18 | 0bab125 | 🔬 Critical Finding: vmaf float_ms_ssim is Y-channel only
* 2026-01-18 | 14c6b7f | 🔧 Add FFmpeg libvmaf installation scripts
* 2026-01-18 | ac03c29 | 🔧 Add FFmpeg libvmaf installation scripts
* 2026-01-18 | a922ef0 | 🔄 Switch to ffmpeg libvmaf priority (now installed)
* 2026-01-18 | aa1150d | 验证ffmpeg libvmaf多通道支持 - 确认MS-SSIM为亮度通道算法
* 2026-01-18 | 98619db | v7.3: 最终验证多层fallback设计科学性
* 2026-01-18 | 7d55c55 | 解释Layer 4为何用SSIM Y而非PSNR
* 2026-01-18 | 19b7810 | 日志分析报告 - 发现5个关键问题
* 2026-01-18 | eb9c116 | v7.4: 修复日志分析发现的问题1/3/4/5
* 2026-01-18 | 4d5c274 | v7.4.1: 改进PNG→JXL管道 + 修复元数据保留
* 2026-01-18 | 326c72f | 重构: 修复 VMAF/MS-SSIM 常量和测试，模块化重复代码
* 2026-01-18 | 30bdeb0 | 修复: 移除脚本中不存在的 --verbose 参数
* 2026-01-18 | 0bb4cf7 | 功能: 添加 verbose 模式支持
* 2026-01-18 | 3e68fc1 | 功能: 保留目录结构 (WIP - imgquality-hevc)
* 2026-01-18 | 4e6e5b8 | 修复: 完成所有工具的 base_dir 支持
* 2026-01-18 | 0b4a310 | 文档: 目录结构保留功能实现状态
* 2026-01-18 | cbdde68 | 修复: 双击脚本正确传递 --recursive 参数
* 2026-01-18 | d253492 | fix: 确认目录结构保留功能正常工作
* 2026-01-18 | 30faafe | fix: 清理过时编译产物并修正双击脚本路径
* 2026-01-18 | caf6a42 | docs: 添加元数据保留功能文档
* 2026-01-18 | 6102f49 | fix: 修复跳过文件复制时不保留目录结构和时间戳的严重BUG
* 2026-01-18 | e203738 | fix: 确保复制文件时保留元数据和合并 XMP
* 2026-01-18 | ba7c9de | 🐛 v7.3.1: Fix directory structure in ALL fallback scenarios
* 2026-01-18 | cecaea9 | ✨ v7.3.2: Modular file copier + Progress bar fix
* 2026-01-18 | 5e142e6 | 🔧 v7.3.3: Smart build system + Binary verification
* 2026-01-18 | 88d607f | 🐛 v7.3.5: Force rebuild + structure verification
* 2026-01-18 | c4ca5d0 | 🚨 v7.4.1: CRITICAL FIX - Use smart_file_copier module
* 2026-01-18 | bc98866 | 🔧 Export preserve_directory_metadata
* 2026-01-18 | cad51c8 | 🚀 v7.4.2: Complete smart_file_copier integration
* 2026-01-18 | 08bee89 | 📝 v7.4 Complete - Directory structure fix
* 2026-01-18 | a48bf4a | 🔧 v7.4.3: Apply smart_copier to vidquality_hevc
* 2026-01-18 | dc46dbf | ✅ v7.4.3: All 4 locations use smart_copier
* 2026-01-18 | 40418c3 | 🔧 v7.4.4: 修复进度条混乱 + smart_build.sh bash 3.x 兼容
* 2026-01-18 | 2fa2783 | 🔧 v7.4.5: 彻底修复文件夹结构BUG - 所有复制点使用 smart_file_copier
* 2026-01-18 | 47bf3ff | 🔧 v7.4.6: 统一四个工具的目录元数据保留
* 2026-01-18 | 9d0099d | 🔧 v7.4.7: 无遗漏设计 - 所有文件类型保留元数据
* 2026-01-18 | 4156d84 | 🔧 v7.4.8: Fix smart_build.sh script - set -e + ((var++)) issue
* 2026-01-18 | 2f15189 | ✅ v7.4.8: Complete metadata preservation audit & fixes
* 2026-01-18 | b180997 | v7.4.9: Output directory timestamp preservation
* 2026-01-18 | 33a4e58 | v7.4.9: FIXED - Output directory timestamp preservation
* 2026-01-18 | 134f6d5 | v7.4.9: FINAL FIX - Directory timestamp preservation after rsync
* 2026-01-18 | bcd0d8a | v7.5.0: File Processing Optimization + Build System Enhancement
* 2026-01-20 | 46c50fa | 🔴 CRITICAL FIX v7.5.1: MS-SSIM freeze for long videos
* 2026-01-20 | efc4d66 | docs: Add v7.5.1 verification script and summary
* 2026-01-20 | 4f85874 | test: Add v7.5.1 freeze fix test scripts and manual test guide
* 2026-01-20 | e7e3644 | test: Add v7.5.1 freeze fix test scripts and manual test guide
* 2026-01-20 | 27fed3e | feat(v7.6.0): MS-SSIM性能优化 - 10倍速度提升
* 2026-01-20 | 7d9893b | feat(v7.6.0): MS-SSIM性能优化 - 10倍速度提升
* 2026-01-20 | fddffdc | 🔥 v7.7: Universal Heartbeat System - Phase 1-3 Complete
* 2026-01-20 | 495a139 | 🔥 v7.7: Universal Heartbeat System - Phase 1-3 Complete
* 2026-01-20 | 82ac353 | 🔥 v7.7: Universal Heartbeat - Phase 2 Complete (Tasks 7-9)
* 2026-01-20 | 04faccb | 🔥 v7.7: Universal Heartbeat - Phase 2 Complete (Tasks 7-9)
* 2026-01-20 | 02d4370 | 🔥 v7.7: Universal Heartbeat - Phase 3 Complete (Tasks 10-12)
* 2026-01-20 | e39a5fa | 🔥 v7.7: Universal Heartbeat - Phase 3 Complete (Tasks 10-12)
* 2026-01-20 | f49d23e | chore: run rustfmt on entire project
* 2026-01-20 | c0eb640 | chore: run rustfmt on entire project
* 2026-01-21 | ab10aed | feat: v7.8 quality improvements - unified logging, modular architecture, zero warnings
* 2026-01-21 | d02a07e | feat: v7.8 quality improvements - unified logging, modular architecture, zero warnings
* 2026-01-21 | d39105f | 🔧 v7.8: 完成容差机制和GIF修复验证
* 2026-01-21 | b91b98c | 🔧 v7.8: 完成容差机制和GIF修复验证
* 2026-01-21 | 2747584 | 🎯 v7.8: 优化容差为1%，符合精确控制理念
* 2026-01-21 | 8bde7fc | 🎯 v7.8: 优化容差为1%，符合精确控制理念
* 2026-01-21 | 4d9e94f | 🔧 v7.8: 修复关键统计BUG - JXL转换应用1%容差机制
* 2026-01-21 | 1ab96be | 🔧 v7.8: 修复关键统计BUG - JXL转换应用1%容差机制
* 2026-01-21 | 84b34f2 | 🔧 v7.8.1: Fix 3 critical BUGs with safe testing
* 2026-01-21 | 04ba240 | 🔧 v7.8.1: Fix 3 critical BUGs with safe testing
* 2026-01-21 | e27b5a8 | 🔧 Fix CJXL large image encoding failure (v7.8.2)
* 2026-01-21 | e4de579 | 🔧 Fix CJXL large image encoding failure (v7.8.2)
* 2026-01-28 | 9eb4733 | fix(scripts): prevent uppercase media files from being copied as non-media
* 2026-01-28 | 14e915f | fix(scripts): prevent uppercase media files from being copied as non-media
* 2026-01-28 | 51d9ece | fix: comprehensive fix for case-insensitive file extension handling across scripts and tools
* 2026-01-28 | f41eff6 | fix: comprehensive fix for case-insensitive file extension handling across scripts and tools
* 2026-01-31 | 64d1b15 | Backup before Anglicization
* 2026-01-31 | 3c91c6d | Backup before Anglicization
* 2026-01-31 | 20a4f68 | Anglicize project: Translate UI, logs, errors and docs to English
* 2026-01-31 | 07d9abf | Anglicize project: Translate UI, logs, errors and docs to English
* 2026-01-31 | 471bc2f | GIF Fix 🐛: proper block parsing; Performance ⚡: Smart thread manager (75% core usage); Rsync 📦: v3.4.1 support; Stability 🛡️: 512MB limit & empty check; Security ✅: 46 command injection patches & case-sensitivity verification
* 2026-01-31 | eafece3 | GIF Fix 🐛: proper block parsing; Performance ⚡: Smart thread manager (75% core usage); Rsync 📦: v3.4.1 support; Stability 🛡️: 512MB limit & empty check; Security ✅: 46 command injection patches & case-sensitivity verification
* 2026-01-31 | 48d1fa7 | fix(conversion,cjxl): reorder cjxl arguments to place flags before files
* 2026-01-31 | 9288b13 | fix(conversion,cjxl): reorder cjxl arguments to place flags before files
* 2026-01-31 | c6c0e0f | fix(tooling): remove unsupported '--' delimiter from ffmpeg, sips, dwebp calls
* 2026-01-31 | 54f4623 | fix(tooling): remove unsupported '--' delimiter from ffmpeg, sips, dwebp calls
* 2026-01-31 | ec5db41 | fix(security): implement strict safe_path_arg wrapper for ffmpeg inputs
* 2026-01-31 | 018c166 | fix(security): implement strict safe_path_arg wrapper for ffmpeg inputs
* 2026-01-31 | 454dc0a | chore: update dependencies and apply security/functional fixes
* 2026-01-31 | adcedc6 | chore: update dependencies and apply security/functional fixes
* 2026-01-31 | f9cfca2 | Update all dependencies to latest versions
* 2026-01-31 | a792751 | Update all dependencies to latest versions
* 2026-01-31 | 431219d | Fix unused import warning in path_safety.rs
* 2026-01-31 | e46cb6a | Fix unused import warning in path_safety.rs
* 2026-01-31 | 1019377 | Fix clippy warnings: doc formatting and io error creation
* 2026-01-31 | 6492e2e | Fix clippy warnings: doc formatting and io error creation
* 2026-02-01 | cdf27e8 | fix: resolve temp file race conditions using tempfile crate (v7.9.2)
* 2026-02-01 | 08f20f0 | fix: resolve temp file race conditions using tempfile crate (v7.9.2)
* 2026-02-01 | 88a4235 | fix(security): comprehensive temp file safety audit and refactor (v7.9.2)
* 2026-02-01 | b6dfb6a | fix(security): comprehensive temp file safety audit and refactor (v7.9.2)
* 2026-02-01 | 88bb7ae | fix(security): replace unreliable extension checks with robust ffprobe content detection (v7.9.3)
* 2026-02-01 | 765ac2f | fix(security): replace unreliable extension checks with robust ffprobe content detection (v7.9.3)
* 2026-02-01 | 15d0a55 | feat(ux): improve logging for fallback copy on conversion failure (v7.9.4)
* 2026-02-01 | 788a600 | feat(ux): improve logging for fallback copy on conversion failure (v7.9.4)
* 2026-02-01 | 83e7e1b | Update files
* 2026-02-01 | 353bd1e | Update files
* 2026-02-01 | 720eb30 | 🛠️ 综合修复与性能优化 / Comprehensive Fixes & Enhancements
* 2026-02-01 | 1610981 | 🛠️ 综合修复与性能优化 / Comprehensive Fixes & Enhancements
* 2026-02-05 | 2d46830 | feat: content-aware format detection and remediation tools for PNG/JPEG mismatch
* 2026-02-05 | b5e8782 | feat: content-aware format detection and remediation tools for PNG/JPEG mismatch
* 2026-02-05 | 58d4124 | v8.0.0: Fix directory structure preservation and enhance content-aware detection
* 2026-02-05 | 7c6bc1d | v8.0.0: Fix directory structure preservation and enhance content-aware detection
* 2026-02-05 | 8a6169e | Cleanup: Remove temporary analysis logs and test artifacts after v8.0.0 release
* 2026-02-05 | 244461c | Cleanup: Remove temporary analysis logs and test artifacts after v8.0.0 release
* 2026-02-07 | acd5ebb | 🔥 v7.9.9: Fix HEIC SecurityLimitExceeded and FFmpeg hang issues
* 2026-02-07 | ca45728 | 🔥 v7.9.9: Fix HEIC SecurityLimitExceeded and FFmpeg hang issues
* 2026-02-07 | 1e5821e | 🔥 v7.9.10: 用心跳检测替代FFmpeg超时机制
* 2026-02-07 | ecd7c5d | 🔥 v7.9.10: 用心跳检测替代FFmpeg超时机制
* 2026-02-07 | 20788aa | 🔥 v7.9.11: Use FfmpegProcess to prevent FFmpeg pipe deadlock
* 2026-02-07 | aed8d0b | 🔥 v7.9.11: Use FfmpegProcess to prevent FFmpeg pipe deadlock
* 2026-02-12 | 3077227 | 🔥 v8.0: Unified Progress Bar & Robustness Overhaul - Created UnifiedProgressBar in shared_utils - Migrated imgquality and video_explorer to unified progress system - Fixed high-risk unwrap() calls in production code - Cleaned up redundant UI path references
* 2026-02-12 | 41e99e7 | 🔥 v8.0: Unified Progress Bar & Robustness Overhaul - Created UnifiedProgressBar in shared_utils - Migrated imgquality and video_explorer to unified progress system - Fixed high-risk unwrap() calls in production code - Cleaned up redundant UI path references
* 2026-02-18 | bd8b27d | Fix pipe buffer deadlock in x265 encoder and update dependencies
* 2026-02-18 | 44c5cf2 | Fix pipe buffer deadlock in x265 encoder and update dependencies
* 2026-02-19 | 449e136 | 清理: 删除110+个临时测试脚本
* 2026-02-19 | 066e524 | 清理: 删除110+个临时测试脚本
* 2026-02-19 | 05408ee | 清理: 删除临时清理脚本
* 2026-02-19 | c357bae | 清理: 删除临时清理脚本
* 2026-02-20 | 2865cde | feat: Add JXL container to codestream converter for iCloud Photos compatibility
* 2026-02-20 | c72c40f | feat: Add JXL container to codestream converter for iCloud Photos compatibility
* 2026-02-20 | 658c584 | feat: Add JXL Container Fix Only mode to UI
* 2026-02-20 | 7210328 | feat: Add JXL Container Fix Only mode to UI
* 2026-02-20 | 245a5b4 | docs: Clarify JXL backup mechanism and add cleanup tool
* 2026-02-20 | 17d5468 | docs: Clarify JXL backup mechanism and add cleanup tool
* 2026-02-20 | d1bfdce | fix: Improve JXL container fixer with organized backups and precise detection
* 2026-02-20 | ca6b7e9 | fix: Improve JXL container fixer with organized backups and precise detection
* 2026-02-20 | 49be22d | fix: Ensure complete metadata preservation following shared_utils pattern
* 2026-02-20 | aafca44 | fix: Ensure complete metadata preservation following shared_utils pattern
* 2026-02-20 | 9985117 | Add Brotli EXIF repair tool
* 2026-02-20 | 7ca8923 | Add Brotli EXIF repair tool
* 2026-02-20 | 2d0aa66 | Improve metadata preservation in Brotli EXIF fix
* 2026-02-20 | 789689f | Improve metadata preservation in Brotli EXIF fix
* 2026-02-20 | 3d5f01c | Add Brotli EXIF corruption prevention to main pipeline
* 2026-02-20 | 9e95a7c | Add Brotli EXIF corruption prevention to main pipeline
* 2026-02-20 | 9642c6d | Revert: Remove -fixBase (ineffective for Brotli corruption)
* 2026-02-20 | e6fec2c | Revert: Remove -fixBase (ineffective for Brotli corruption)
* 2026-02-20 | 945f5a1 | Fix: Remove -all:all from XMP merge to prevent Brotli corruption
* 2026-02-20 | 23d5570 | Fix: Remove -all:all from XMP merge to prevent Brotli corruption
* 2026-02-20 | e264b9b | docs: clarify design decision to keep -all:all for maximum information preservation
* 2026-02-20 | 2f834ee | docs: clarify design decision to keep -all:all for maximum information preservation
* 2026-02-20 | ca01052 | fix: preserve DateCreated in Brotli EXIF repair without re-introducing corruption
* 2026-02-20 | 3304a87 | fix: preserve DateCreated in Brotli EXIF repair without re-introducing corruption
* 2026-02-20 | 46c8be8 | feat: add Brotli EXIF Fix option to drag-and-drop menu
* 2026-02-20 | 655d24d | feat: add Brotli EXIF Fix option to drag-and-drop menu
* 2026-02-20 | c7c83b7 | refactor: remove imprecise JXL Container Fix option
* 2026-02-20 | 30b62a6 | refactor: remove imprecise JXL Container Fix option
* 2026-02-20 | 2120ef4 | fix: improve file iteration reliability in Brotli EXIF fix script
* 2026-02-20 | 516fb12 | fix: improve file iteration reliability in Brotli EXIF fix script
* 2026-02-20 | eefd11d | fix: add -warning flag to exiftool for reliable Brotli detection
* 2026-02-20 | 7c73902 | fix: add -warning flag to exiftool for reliable Brotli detection
* 2026-02-20 | a26cb37 | 🔒 元数据安全性修复：金标准重构 + 源头预防 Brotli 损坏
* 2026-02-20 | d4ccdd1 | 🔒 元数据安全性修复：金标准重构 + 源头预防 Brotli 损坏
* 2026-02-20 | 09bf9a1 | 🍎 Apple 兼容模式条件化修复：Brotli 元数据损坏问题 100% 解决
* 2026-02-20 | 0be816c | 🍎 Apple 兼容模式条件化修复：Brotli 元数据损坏问题 100% 解决
* 2026-02-20 | a453d7b | Enhance HEIC detection and smart correction handling
* 2026-02-20 | cc60fd8 | Enhance HEIC detection and smart correction handling
* 2026-02-20 | f2bdbd9 | Fix: Content-aware extension correction and on-demand structural repair
* 2026-02-20 | c0731f0 | Fix: Content-aware extension correction and on-demand structural repair
* 2026-02-20 | f3e7724 | Fix: Replace all Chinese text with English
* 2026-02-20 | a18b618 | Fix: Replace all Chinese text with English
* 2026-02-20 | 5dd957c | Fix: Add ImageMagick identify fallback for WebP/GIF animation duration
* 2026-02-20 | 320876d | Fix: Add ImageMagick identify fallback for WebP/GIF animation duration
* 2026-02-20 | 4f036fa | Update dependencies to latest versions
* 2026-02-20 | c30877d | Update dependencies to latest versions
* 2026-02-20 | bfae170 | Update dependencies: tempfile 3.20, proptest 1.7
* 2026-02-20 | 8870967 | Update dependencies: tempfile 3.20, proptest 1.7
* 2026-02-20 | 2fd9e52 | Merge remote merge/v5.2-v5.54-gentle
* 2026-02-20 | a647fca | Merge remote merge/v5.2-v5.54-gentle
* 2026-02-20 | d2501a0 | Fix: Replace remaining Chinese error messages with English
* 2026-02-20 | a1a7632 | Fix: Replace remaining Chinese error messages with English
* 2026-02-21 | fb397b1 | fix: Deep audit — 12 bug fixes across extension handling, pipelines, and tooling
* 2026-02-21 | bb46646 | fix: Deep audit — 12 bug fixes across extension handling, pipelines, and tooling
* 2026-02-21 | c7af6b4 | fix: Systematic code quality sweep — clippy, safety, error visibility
* 2026-02-21 | bbbfd3d | fix: Systematic code quality sweep — clippy, safety, error visibility
* 2026-02-21 | cf7cd47 | feat: 添加完整会话日志记录功能
* 2026-02-21 | 8f72113 | feat: 添加完整会话日志记录功能
* 2026-02-21 | 9b63d63 | chore: maintainability and deduplication (plan)
* 2026-02-21 | 3f4c31e | chore: maintainability and deduplication (plan)
* 2026-02-21 | 3114be1 | feat: GIF 响亮报错+无遗漏设计(相邻目录)+校准stderr
* 2026-02-21 | 6057fd5 | feat: GIF 响亮报错+无遗漏设计(相邻目录)+校准stderr
* 2026-02-21 | a1e5a13 | fix(calibration): GIF 使用 FFmpeg 单步 libx265 校准，避免 Y4M→x265 管道失败
* 2026-02-21 | fa46a4d | fix(calibration): GIF 使用 FFmpeg 单步 libx265 校准，避免 Y4M→x265 管道失败
* 2026-02-21 | 983e03d | 🚀 Refactor: Simplification of project structure and dependencies
* 2026-02-21 | 3d9494f | 🚀 Refactor: Simplification of project structure and dependencies
* 2026-02-21 | ef79ae3 | 🧹 Maintenance: Centralize build artifacts to root target directory
* 2026-02-21 | 9027f14 | 🧹 Maintenance: Centralize build artifacts to root target directory
* 2026-02-21 | 2d1629b | 🎨 Audit: Unified code style and syntax fixes
* 2026-02-21 | f386dc8 | 🎨 Audit: Unified code style and syntax fixes
* 2026-02-22 | edd451d | 📦 Refactor: Extract image and video analysis logic to shared_utils
* 2026-02-22 | 395e466 | 📦 Refactor: Extract image and video analysis logic to shared_utils
* 2026-02-22 | aaa029b | Fix recursive directory processing consistency across all tools, restore JXL extension support in file copier, and add directory analysis support to video tools.
* 2026-02-22 | 81437be | Fix recursive directory processing consistency across all tools, restore JXL extension support in file copier, and add directory analysis support to video tools.
* 2026-02-22 | da41be4 | Complete consistency sweep: add allow_size_tolerance and no_allow_size_tolerance to all AV1 tools for full parity with HEVC tools.
* 2026-02-22 | 017c254 | Complete consistency sweep: add allow_size_tolerance and no_allow_size_tolerance to all AV1 tools for full parity with HEVC tools.
* 2026-02-22 | 64895c2 | Replace standalone JXL fixer with unified Apple Photos repair script in drag_and_drop_processor.sh.
* 2026-02-22 | 7802ea4 | Replace standalone JXL fixer with unified Apple Photos repair script in drag_and_drop_processor.sh.
* 2026-02-22 | 26c6518 | Complete AV1 tools alignment: Finalize img_av1 and vid_av1 with parity to HEVC counterparts, including apple_compat, ultimate flags, MS-SSIM enhancements, and improved metadata/stats tracking.
* 2026-02-22 | 818eee8 | Complete AV1 tools alignment: Finalize img_av1 and vid_av1 with parity to HEVC counterparts, including apple_compat, ultimate flags, MS-SSIM enhancements, and improved metadata/stats tracking.
* 2026-02-22 | ba65b3d | Fix(video_explorer): Refine GIF verification logic in Phase 3.
* 2026-02-22 | 56a14ff | Fix(video_explorer): Refine GIF verification logic in Phase 3.
* 2026-02-23 | b6e91a0 | refactor(shared_utils): remove unused simple_progress and realtime_progress modules
* 2026-02-23 | 7a0b92d | refactor(shared_utils): remove unused simple_progress and realtime_progress modules
* 2026-02-23 | 8e7646f | chore: strip all inline comments, keep only module-level //! docs
* 2026-02-23 | b422109 | chore: strip all inline comments, keep only module-level //! docs
* 2026-02-23 | 582826d | fix: audit fixes + modernization
* 2026-02-23 | cb71337 | fix: audit fixes + modernization
* 2026-02-23 | 10dae3a | refactor: deduplicate ConversionResult boilerplate, bump to v8.4.0
* 2026-02-23 | 799044a | refactor: deduplicate ConversionResult boilerplate, bump to v8.4.0
* 2026-02-23 | 2f6c761 | fix: SSIM 计算失败、安全性增强与代码健壮性修复
* 2026-02-23 | d6347ac | fix: SSIM 计算失败、安全性增强与代码健壮性修复
* 2026-02-23 | 47c69f8 | refactor: 代码质量改进 — 抽象重复模式，消除冗余代码 (-100 net lines)
* 2026-02-23 | 85f6e57 | refactor: 代码质量改进 — 抽象重复模式，消除冗余代码 (-100 net lines)
* 2026-02-23 | fc8b360 | refactor: 代码重构与 Alpha 通道修复
* 2026-02-23 | 0e3e91c | refactor: 代码重构与 Alpha 通道修复
* 2026-02-23 | c17c16b | refactor: split video_explorer.rs into focused submodules
* 2026-02-23 | 0b286f1 | refactor: split video_explorer.rs into focused submodules
* 2026-02-23 | f29f3d5 | test: remove 26 low-value tests across 3 files
* 2026-02-23 | b8c8930 | test: remove 26 low-value tests across 3 files
* 2026-02-23 | 986078a | scripts: add spinner and elapsed time at bottom during processing
* 2026-02-23 | 7206735 | scripts: add spinner and elapsed time at bottom during processing
* 2026-02-23 | f6163c4 | scripts: keep spinner out of session log; use tee for logging
* 2026-02-23 | 61aa93b | scripts: keep spinner out of session log; use tee for logging
* 2026-02-23 | 77b4823 | Apple compat fallback: explicit report, keep last best-effort attempt
* 2026-02-23 | b1dfe81 | Apple compat fallback: explicit report, keep last best-effort attempt
* 2026-02-23 | 41e1d53 | Ultimate mode: widen attempt counts and raise saturation/fallback limits
* 2026-02-23 | 1c37e3a | Ultimate mode: widen attempt counts and raise saturation/fallback limits
* 2026-02-23 | 41907f6 | fix(log): per-file log context to fix interleaved output
* 2026-02-23 | 0057fad | fix(log): per-file log context to fix interleaved output
* 2026-02-23 | 6d25b5b | fix(log): UTF-8-safe prefix, formatted indentation for all log lines
* 2026-02-23 | 96ab6b2 | fix(log): UTF-8-safe prefix, formatted indentation for all log lines
* 2026-02-23 | 989b785 | fix(duration): ImageMagick fallback when ffprobe has no duration for WebP/GIF
* 2026-02-23 | 691ce6e | fix(duration): ImageMagick fallback when ffprobe has no duration for WebP/GIF
* 2026-02-24 | 597e817 | fix(log): clearer QualityCheck for GIF when verification skipped
* 2026-02-24 | 5b3bb4e | fix(log): clearer QualityCheck for GIF when verification skipped
* 2026-02-24 | c971389 | Release 8.5.0: logging, duration fallback, GIF quality verification
* 2026-02-24 | 30bde9c | Release 8.5.0: logging, duration fallback, GIF quality verification
* 2026-02-24 | 0ec267e | audit: path safety, div-by-zero, unsafe comments, doc
* 2026-02-24 | bc23142 | audit: path safety, div-by-zero, unsafe comments, doc
* 2026-02-24 | 95e9e67 | feat: log_file, XMP progress, i18n and test gating
* 2026-02-24 | 41d54d4 | feat: log_file, XMP progress, i18n and test gating
* 2026-02-24 | 2374f78 | audit: path safety for video_explorer SSIM/PSNR/MS-SSIM and dynamic_mapping calibration
* 2026-02-24 | 28409aa | audit: path safety for video_explorer SSIM/PSNR/MS-SSIM and dynamic_mapping calibration
* 2026-02-24 | 6bcbe4f | audit: img_av1/img_hevc path safety and div-by-zero
* 2026-02-24 | 87eee91 | audit: img_av1/img_hevc path safety and div-by-zero
* 2026-02-24 | 5585f6f | audit: logic/math/ordering — div-by-zero and numeric safety
* 2026-02-24 | 3e3022f | audit: logic/math/ordering — div-by-zero and numeric safety
* 2026-02-24 | 79f7ee1 | audit: image_metrics.rs SSIM/MS-SSIM correctness and perf
* 2026-02-24 | 0117346 | audit: image_metrics.rs SSIM/MS-SSIM correctness and perf
* 2026-02-24 | 19b719d | audit: image_quality_core.rs safety, correctness, and design
* 2026-02-24 | 5000747 | audit: image_quality_core.rs safety, correctness, and design
* 2026-02-24 | 72f3b22 | audit: img_av1 conversion_api.rs correctness and design
* 2026-02-24 | f929646 | audit: img_av1 conversion_api.rs correctness and design
* 2026-02-24 | 68e4aea | audit: img_av1 lossless_converter.rs correctness and cleanup
* 2026-02-24 | 121ed2d | audit: img_av1 lossless_converter.rs correctness and cleanup
* 2026-02-24 | a72ba02 | video_explorer: audit fixes (GSS naming, SSIM comment, build(), confidence, prop test, best_crf_so_far)
* 2026-02-24 | 40c2856 | video_explorer: audit fixes (GSS naming, SSIM comment, build(), confidence, prop test, best_crf_so_far)
* 2026-02-24 | fdd19ed | AVIF/AV1 health check + gpu_accel audit fixes
* 2026-02-24 | 89aa028 | AVIF/AV1 health check + gpu_accel audit fixes
* 2026-02-24 | b9b1567 | explore_strategy audit: binary_search_quality goal, compress Option, PSNR→SSIM, proptest, docs
* 2026-02-24 | 6dd0475 | explore_strategy audit: binary_search_quality goal, compress Option, PSNR→SSIM, proptest, docs
* 2026-02-24 | 68ed74b | video_quality_detector audit: content type, fallbacks, chroma, routing, bpp, CRF
* 2026-02-24 | fba4d3b | video_quality_detector audit: content type, fallbacks, chroma, routing, bpp, CRF
* 2026-02-24 | 0db1287 | Implement quality_verifier_enhanced; heartbeat; progress/gpu fixes; audit
* 2026-02-24 | fa9af22 | Implement quality_verifier_enhanced; heartbeat; progress/gpu fixes; audit
* 2026-02-24 | a476a17 | Use quality_verifier_enhanced in pipeline; re-export; code quality
* 2026-02-24 | b0865a3 | Use quality_verifier_enhanced in pipeline; re-export; code quality
* 2026-02-24 | 2af4cc4 | Fix metadata/windows.rs invalid escape; cargo fmt --all
* 2026-02-24 | 978fdee | Fix metadata/windows.rs invalid escape; cargo fmt --all
* 2026-02-24 | b4267d8 | docs: metadata/network.rs purpose, CODE_AUDIT §24, deps note
* 2026-02-24 | dd24636 | docs: metadata/network.rs purpose, CODE_AUDIT §24, deps note
* 2026-02-24 | 0c54f7e | Ultimate mode: raise Domain Wall required zero-gains to 15–20
* 2026-02-24 | f26fad8 | Ultimate mode: raise Domain Wall required zero-gains to 15–20
* 2026-02-24 | 78dd940 | docs: CODE_AUDIT updates; precheck/dynamic_mapping/ssim/precision/stream_analysis fixes
* 2026-02-24 | bdb1bc4 | docs: CODE_AUDIT updates; precheck/dynamic_mapping/ssim/precision/stream_analysis fixes
* 2026-02-24 | 17612d5 | logging: unify stderr output with tracing (CODE_AUDIT)
* 2026-02-24 | 1986e18 | logging: unify stderr output with tracing (CODE_AUDIT)
* 2026-02-24 | b3e9073 | docs: CODE_AUDIT §28.2 log format; logging: align file lines, stderr indent, LOG_TAG_WIDTH 24
* 2026-02-24 | fd90ebf | docs: CODE_AUDIT §28.2 log format; logging: align file lines, stderr indent, LOG_TAG_WIDTH 24
* 2026-02-24 | e9c4256 | shared_utils: video_explorer codec_detection.rs updates
* 2026-02-24 | 13d88e9 | shared_utils: video_explorer codec_detection.rs updates
* 2026-02-24 | 92e66a9 | fix(logging): strip ANSI from file log output so files are plain text
* 2026-02-24 | 602a7ca | fix(logging): strip ANSI from file log output so files are plain text
* 2026-02-24 | 1fdd9e1 | fix(logging): strip ANSI when stderr not TTY, quiet GPU line on terminal
* 2026-02-24 | 7842ecf | fix(logging): strip ANSI when stderr not TTY, quiet GPU line on terminal
* 2026-02-24 | 7159717 | fix(img): unify compress check for all image conversion paths
* 2026-02-24 | 8f13b7b | fix(img): unify compress check for all image conversion paths
* 2026-02-24 | d92132e | fix(audit): CLI duplication + pipe error handling
* 2026-02-24 | 57bfd0b | fix(audit): CLI duplication + pipe error handling
* 2026-02-24 | 5907272 | fix(audit): GPU concurrency limit + VAAPI device configurable
* 2026-02-24 | 3dff5ac | fix(audit): GPU concurrency limit + VAAPI device configurable
* 2026-02-24 | 0f0771d | Audit fixes: GIF parser bounds check, rsync via which, processed list file lock (Unix)
* 2026-02-24 | 0f598f7 | Audit fixes: GIF parser bounds check, rsync via which, processed list file lock (Unix)
* 2026-02-24 | a9e8cef | chore: bump version to 8.6.0
* 2026-02-24 | a2712f1 | chore: bump version to 8.6.0
* 2026-02-24 | 03e8d50 | chore(deps): bump libheif-rs to 2.6.1, tempfile to 3.26
* 2026-02-24 | ca76f86 | chore(deps): bump libheif-rs to 2.6.1, tempfile to 3.26
* 2026-02-24 | b7f69f0 | fix(video_explorer): heuristic early-exit sensitivity for flat bitrate curves
* 2026-02-24 | eda4074 | fix(video_explorer): heuristic early-exit sensitivity for flat bitrate curves
* 2026-02-24 | 5a93713 | feat(ssim): increase segment duration for better media-type adaptation
* 2026-02-24 | fdd2d84 | feat(ssim): increase segment duration for better media-type adaptation
* 2026-02-24 | a994b7b | feat(ultimate): longer SSIM segment duration in ultimate mode
* 2026-02-24 | 6ef1a35 | feat(ultimate): longer SSIM segment duration in ultimate mode
* 2026-02-24 | 442500a | fix(progress): XMP merge display single count, no OK/N to avoid confusion with Metadata total
* 2026-02-24 | 0e2a033 | fix(progress): XMP merge display single count, no OK/N to avoid confusion with Metadata total
* 2026-02-24 | 0a10d9d | feat(progress): Images OK/failed on same line as XMP/JXL; image milestones
* 2026-02-24 | 19e5dad | feat(progress): Images OK/failed on same line as XMP/JXL; image milestones
* 2026-02-24 | a40b2d0 | fix(conversion): merge XMP sidecar into converted output (real flow fix, not display only)
* 2026-02-24 | d29dcab | fix(conversion): merge XMP sidecar into converted output (real flow fix, not display only)
* 2026-02-25 | 10d5b75 | Audit follow-up: document Phase 2 assumption, iteration cap, efficiency factors; warn when MS-SSIM skipped for long video (8.5.1)
* 2026-02-25 | 6f60725 | Audit follow-up: document Phase 2 assumption, iteration cap, efficiency factors; warn when MS-SSIM skipped for long video (8.5.1)
* 2026-02-25 | 0bee838 | Ultimate mode: use 25min MS-SSIM skip threshold (8.5.2)
* 2026-02-25 | d388792 | Ultimate mode: use 25min MS-SSIM skip threshold (8.5.2)
* 2026-02-25 | 560cb38 | quality_matcher: defensive design for extreme BPP (NaN/Inf, clamp to safe range, CRF clamp); CODE_AUDIT §35
* 2026-02-25 | c397a47 | quality_matcher: defensive design for extreme BPP (NaN/Inf, clamp to safe range, CRF clamp); CODE_AUDIT §35
* 2026-02-25 | f568b54 | GIF: exclude from Apple compat fallback (fail = copy original only); add docs/COPY_AND_COMPLETENESS.md (copy strategy, no-omission, conflicts)
* 2026-02-25 | e6b7f4a | GIF: exclude from Apple compat fallback (fail = copy original only); add docs/COPY_AND_COMPLETENESS.md (copy strategy, no-omission, conflicts)
* 2026-02-25 | f3a3221 | Copy strategy & extension fix: doc §36; video path fix-before-validate; no rsync; 动图不混淆
* 2026-02-25 | 18734a5 | Copy strategy & extension fix: doc §36; video path fix-before-validate; no rsync; 动图不混淆
* 2026-02-26 | d102a8a | doc: 扩展名修正不混淆动图；检测顺序保证 GIF/WebP/AVIF 先于视频
* 2026-02-26 | b5b8266 | doc: 扩展名修正不混淆动图；检测顺序保证 GIF/WebP/AVIF 先于视频
* 2026-02-26 | 396679f | Round 4 audit fixes: img_hevc path/config, vid_hevc quality+static GIF, docs
* 2026-02-26 | bddfece | Round 4 audit fixes: img_hevc path/config, vid_hevc quality+static GIF, docs
* 2026-02-26 | 78757b5 | img_hevc: pass full config to convert_to_jxl, add output verification and compress check
* 2026-02-26 | 31d2e1c | img_hevc: pass full config to convert_to_jxl, add output verification and compress check
* 2026-02-26 | e4d8e30 | Unify user-facing errors and logs to English; CODE_AUDIT §38.11
* 2026-02-26 | 759b327 | Unify user-facing errors and logs to English; CODE_AUDIT §38.11
* 2026-02-26 | 5f8d3b0 | Align img_av1 and vid_av1 with hevc tools (§38.8)
* 2026-02-26 | 577e37b | Align img_av1 and vid_av1 with hevc tools (§38.8)
* 2026-02-26 | 9108c7c | TOCTOU mitigation: temp file + atomic rename in conversion APIs
* 2026-02-26 | bd17c1d | TOCTOU mitigation: temp file + atomic rename in conversion APIs
* 2026-02-26 | a8aeea2 | fix: implement TOCTOU-safe conversion and address design audit findings for HEVC/AV1 modules
* 2026-02-26 | bc13c31 | fix: implement TOCTOU-safe conversion and address design audit findings for HEVC/AV1 modules
* 2026-02-27 | 96010d2 | Fix libheif deprecation; document match _ pattern (CODE_AUDIT §39)
* 2026-02-27 | 8d3d781 | Fix libheif deprecation; document match _ pattern (CODE_AUDIT §39)
* 2026-02-27 | 7a88ba5 | Temp path fix (stem.tmp.ext) + re-audit doc
* 2026-02-27 | 24791ec | Temp path fix (stem.tmp.ext) + re-audit doc
* 2026-02-27 | b3c3798 | audit: remove redundant config, unify logging, align vid_hevc/vid_av1
* 2026-02-27 | d08f40d | audit: remove redundant config, unify logging, align vid_hevc/vid_av1
* 2026-02-27 | fa494d8 | jxl/imagemagick: better diagnostics and format-agnostic animation log
* 2026-02-27 | 4a8e1b1 | jxl/imagemagick: better diagnostics and format-agnostic animation log
* 2026-02-27 | 9b79503 | script: do not auto-run Apple Photos Compatibility Repair; jxl: strip only on grayscale+ICC retry, document metadata preservation
* 2026-02-27 | 3a6db8f | script: do not auto-run Apple Photos Compatibility Repair; jxl: strip only on grayscale+ICC retry, document metadata preservation
* 2026-02-27 | 579f7a1 | script: disable auto Apple Photos repair; app: confirm before run (double-click)
* 2026-02-27 | 3931477 | script: disable auto Apple Photos repair; app: confirm before run (double-click)
* 2026-02-27 | aaeb4e0 | release: v8.7.0 - Critical bug fixes and comprehensive audit completion
* 2026-02-27 | 0945f43 | release: v8.7.0 - Critical bug fixes and comprehensive audit completion
* 2026-02-27 | 380de00 | fix: spinner Killed:9 suppression, negative elapsed time, pipeline failed filename
* 2026-02-27 | c0fe5b6 | fix: spinner Killed:9 suppression, negative elapsed time, pipeline failed filename
* 2026-02-27 | e1dd5b6 | feat(video): expand codec scope, strict ProRes/DNxHD, fallback only for Apple-incompatible
* 2026-02-27 | 47e72d6 | feat(video): expand codec scope, strict ProRes/DNxHD, fallback only for Apple-incompatible
* 2026-02-27 | a7bdf89 | fix(video): normal mode skip AV1/VP9/VVC/AV2; only Apple-compat converts them
* 2026-02-27 | 6d0a340 | fix(video): normal mode skip AV1/VP9/VVC/AV2; only Apple-compat converts them
* 2026-02-27 | dd84f02 | feat(animated): raise min duration for animated→video to 4.5s
* 2026-02-27 | c88be8b | feat(animated): raise min duration for animated→video to 4.5s
* 2026-02-27 | 74bcb41 | 从 y4m 直连到内存管控：管道防堵、日志去噪、OOM 防护
* 2026-02-27 | 9c44839 | 从 y4m 直连到内存管控：管道防堵、日志去噪、OOM 防护
* 2026-02-27 | a21a1ae | fix: 会话日志完整录制 img-hevc/vid-hevc 输出（含 stderr）
* 2026-02-27 | a5e0b3f | fix: 会话日志完整录制 img-hevc/vid-hevc 输出（含 stderr）
* 2026-02-27 | d04006b | chore: 依赖更新至最新兼容版本
* 2026-02-27 | b9c6cb9 | chore: 依赖更新至最新兼容版本
* 2026-02-28 | 714372a | feat(image): 图像质量判断可靠性改进与转换逻辑审计
* 2026-02-28 | 9824a6a | feat(image): 图像质量判断可靠性改进与转换逻辑审计
* 2026-02-28 | dc049a1 | Audit fixes: P0-2/D1-D6 — compress doc, tolerance doc, safe_delete constants, Apple fallback predicate, reject empty commit, temp+commit doc, phase comments
* 2026-02-28 | dbcc77e | Audit fixes: P0-2/D1-D6 — compress doc, tolerance doc, safe_delete constants, Apple fallback predicate, reject empty commit, temp+commit doc, phase comments
* 2026-02-28 | b714797 | Apple fallback: behavior by total file size only; video stream stays internal
* 2026-02-28 | 69bf43a | Apple fallback: behavior by total file size only; video stream stays internal
* 2026-02-28 | 429157b | feat(img): resume from last run (--resume/--no-resume), doc updates
* 2026-02-28 | 76c21d1 | feat(img): resume from last run (--resume/--no-resume), doc updates
* 2026-02-28 | 21e1363 | chore: push remaining changes (image_quality_core removal, pixel routing doc, default log, audit)
* 2026-02-28 | 0f50fdd | chore: push remaining changes (image_quality_core removal, pixel routing doc, default log, audit)
* 2026-02-28 | 3715778 | fix: default run logs go to ./logs/ (gitignored), add *_run.log to .gitignore
* 2026-02-28 | be9ee0b | fix: default run logs go to ./logs/ (gitignored), add *_run.log to .gitignore
* 2026-02-28 | e6dbf8a | image: AVIF format-level is_lossless + pixel fallback; doc reliability and fallback checklist
* 2026-02-28 | 6ba6303 | image: AVIF format-level is_lossless + pixel fallback; doc reliability and fallback checklist
* 2026-02-28 | 05ba56c | anim: WebP native duration parse + retry when duration unknown, no fake default
* 2026-02-28 | ce74832 | anim: WebP native duration parse + retry when duration unknown, no fake default
* 2026-02-28 | 9edd55e | fix: 修复伪造成功/日志/验证/XMP 等多项问题并更新审计文档
* 2026-02-28 | a827afc | fix: 修复伪造成功/日志/验证/XMP 等多项问题并更新审计文档
* 2026-02-28 | 764ee54 | fix: resolve file logging issues - merge run logs into session log, flush after critical writes
* 2026-02-28 | f969d0f | fix: resolve file logging issues - merge run logs into session log, flush after critical writes
* 2026-02-28 | ae8f955 | fix: 日志全部即时落盘 + vid_hevc 默认 run log
* 2026-02-28 | fb6c244 | fix: 日志全部即时落盘 + vid_hevc 默认 run log
* 2026-02-28 | 45abd73 | fix: 默认 run log 文件名加时间戳，避免多次运行重名冲突
* 2026-02-28 | 0461587 | fix: 默认 run log 文件名加时间戳，避免多次运行重名冲突
* 2026-02-28 | 56971fa | log: 移除 --log-file，自动命名并始终写 run log；run log 全量未过滤+emoji；日志文件加 advisory 锁
* 2026-02-28 | 1fae2f6 | log: 移除 --log-file，自动命名并始终写 run log；run log 全量未过滤+emoji；日志文件加 advisory 锁
* 2026-02-28 | 5e0a795 | logging: make log level apply to direct run-log writes (should_log + write_to_log_at_level)
* 2026-02-28 | f1327d2 | logging: make log level apply to direct run-log writes (should_log + write_to_log_at_level)
* 2026-02-28 | 2e8dc3f | quality: surface enhanced verify failure reason + regression tests (temp-copy only, no pollute)
* 2026-02-28 | 743ee5e | quality: surface enhanced verify failure reason + regression tests (temp-copy only, no pollute)
* 2026-02-28 | 49bb4d4 | chore: bump version to 0.8.8, adopt 0.8.x scheme, update docs
* 2026-02-28 | e21e2a0 | chore: bump version to 0.8.8, adopt 0.8.x scheme, update docs
* 2026-02-28 | e927749 | chore: cargo update (js-sys, redox_syscall, wasm-bindgen); add release notes file; include pending doc/code changes
* 2026-02-28 | 03ab292 | chore: cargo update (js-sys, redox_syscall, wasm-bindgen); add release notes file; include pending doc/code changes
* 2026-02-28 | 64bab40 | fix: remove ExifTool _exiftool_tmp before merge to avoid 'Temporary file already exists'; merge PRE-PROCESSING output to one line
* 2026-02-28 | 1e873f5 | fix: remove ExifTool _exiftool_tmp before merge to avoid 'Temporary file already exists'; merge PRE-PROCESSING output to one line
* 2026-02-28 | d51dea0 | app: 30min timeout for folder picker and confirm dialog; close and exit on timeout
* 2026-02-28 | 47cd243 | app: 30min timeout for folder picker and confirm dialog; close and exit on timeout
* 2026-02-28 | d3b06f1 | app: use AppleScript entry (osascript) to avoid extra terminal window and 'terminate zsh' prompt
* 2026-02-28 | 34d186c | app: use AppleScript entry (osascript) to avoid extra terminal window and 'terminate zsh' prompt
* 2026-02-28 | 5922e48 | app: use zsh instead of bash; run Terminal osascript in background then exit to avoid two windows and 'terminate process' warning
* 2026-02-28 | 63e9ade | app: use zsh instead of bash; run Terminal osascript in background then exit to avoid two windows and 'terminate process' warning
* 2026-02-28 | cc14b5a | app: do not close any Terminal window; avoid closing user manual window or wrong window when multi-opening
* 2026-02-28 | c92dfd6 | app: do not close any Terminal window; avoid closing user manual window or wrong window when multi-opening
* 2026-02-28 | 4b30abd | logs: add Fallback count to status line; run log always full; less noise
* 2026-02-28 | d3cb752 | logs: add Fallback count to status line; run log always full; less noise
* 2026-02-28 | 66be686 | scripts: put Output to Adjacent first; drain stdin between stages for safe prompts
* 2026-02-28 | e962d89 | scripts: put Output to Adjacent first; drain stdin between stages for safe prompts
* 2026-03-01 | 8a03909 | video: unify compression decision on total file size with video-stream diagnostics
* 2026-03-01 | 127aded | video: unify compression decision on total file size with video-stream diagnostics
* 2026-03-01 | 31a17ea | Fix: apple_compat flag in ImageMagick fallback + cjxl decode error retry
* 2026-03-01 | a67cfb0 | Fix: apple_compat flag in ImageMagick fallback + cjxl decode error retry
* 2026-03-01 | 8fbfe10 | Release v0.8.9
* 2026-03-01 | 0b24a24 | Release v0.8.9
* 2026-03-01 | 5c70370 | docs: add code quality audit results to CHANGELOG for v0.8.9
* 2026-03-01 | 8e7c3ef | docs: add code quality audit results to CHANGELOG for v0.8.9
* 2026-03-01 | 3ecd9f1 | docs: add performance optimization to CHANGELOG for v0.8.9
* 2026-03-01 | e789201 | docs: add performance optimization to CHANGELOG for v0.8.9
* 2026-03-02 | c8d63ff | Add subtitle and audio channel support for MKV/MP4 containers
* 2026-03-02 | a3ea725 | Add subtitle and audio channel support for MKV/MP4 containers
* 2026-03-02 | c26fd85 | feat: 实现 HDR 图像保留功能
* 2026-03-02 | 02cc3f2 | feat: 实现 HDR 图像保留功能
* 2026-03-02 | 13400c6 | feat: Add Live Photo detection and skip in Apple compat mode
* 2026-03-02 | b1f0cad | feat: Add Live Photo detection and skip in Apple compat mode
* 2026-03-02 | e6a0052 | docs: improve README.md with detailed technical architecture and update libheif-rs
* 2026-03-02 | 2cc63cb | docs: improve README.md with detailed technical architecture and update libheif-rs
* 2026-03-02 | 46b58e9 | fix: use portable bash shebang in drag_and_drop_processor.sh
* 2026-03-02 | 836a79b | fix: use portable bash shebang in drag_and_drop_processor.sh
* 2026-03-02 | e76b43f | i18n: translate all shell scripts to English
* 2026-03-02 | 1b776ab | i18n: translate all shell scripts to English
* 2026-03-02 | 0f9fd67 | Add Dolby Vision (DV) support with dovi_tool integration
* 2026-03-02 | 47d3903 | Add Dolby Vision (DV) support with dovi_tool integration
* 2026-03-02 | 4ddd4cc | feat: Add HEIC HDR/Dolby Vision detection and skip
* 2026-03-02 | 71a228f | feat: Add HEIC HDR/Dolby Vision detection and skip
* 2026-03-03 | 16585ae | feat: ultimate mode 3D quality gate (VMAF-Y + CAMBI + PSNR-UV)
* 2026-03-03 | 7e4baa5 | feat: ultimate mode 3D quality gate (VMAF-Y + CAMBI + PSNR-UV)
* 2026-03-03 | d32b9f8 | fix: relax duration tolerance for animated images (GIF/WebP/AVIF)
* 2026-03-03 | 3454eed | fix: relax duration tolerance for animated images (GIF/WebP/AVIF)
* 2026-03-03 | 1cf0163 | feat: GIF multi-dimensional meme-score to replace duration-only skip logic
* 2026-03-03 | 323f188 | feat: GIF multi-dimensional meme-score to replace duration-only skip logic
* 2026-03-03 | 515a31b | fix: resolve clippy warnings in gif_meme_score and animated_image
* 2026-03-03 | 248bf26 | fix: resolve clippy warnings in gif_meme_score and animated_image
* 2026-03-03 | 64a7ffb | feat: GIF judgment — five-layer edge-case suppression strategy
* 2026-03-03 | fb03a6c | feat: GIF judgment — five-layer edge-case suppression strategy
* 2026-03-03 | bd425fb | fix: CAMBI calculation broken — libvmaf requires two inputs
* 2026-03-03 | 9626065 | fix: CAMBI calculation broken — libvmaf requires two inputs
* 2026-03-03 | b728f7f | release: v0.9.0 — fix CAMBI 3D gate, tighten thresholds, consolidate docs
* 2026-03-03 | c3b61a4 | release: v0.9.0 — fix CAMBI 3D gate, tighten thresholds, consolidate docs
* 2026-03-03 | 0b537d0 | fix(img-hevc): replace outdated 4.5s duration cutoff with meme-score for GIF
* 2026-03-03 | 197bc8d | fix(img-hevc): replace outdated 4.5s duration cutoff with meme-score for GIF
* 2026-03-03 | ea7573a | Fix: Improve grayscale PNG + RGB ICC profile error detection
* 2026-03-03 | 2c1a26f | Fix: Improve grayscale PNG + RGB ICC profile error detection
* 2026-03-03 | 6311aba | Fix: Skip palette-quantized (lossy) PNG to avoid generational loss
* 2026-03-03 | 9b55458 | Fix: Skip palette-quantized (lossy) PNG to avoid generational loss
* 2026-03-03 | cbf1011 | Fix: Lossy PNG → JXL d=1.0 (try compress, skip if larger); update README
* 2026-03-03 | 15e796c | Fix: Lossy PNG → JXL d=1.0 (try compress, skip if larger); update README
* 2026-03-03 | 702ac0c | Fix: Suppress spurious 'ExifTool failed: ' warnings when stderr is empty
* 2026-03-03 | 158f0f3 | Fix: Suppress spurious 'ExifTool failed: ' warnings when stderr is empty
* 2026-03-03 | 12f2407 | ci: add GitHub Actions workflow for cross-platform release builds
* 2026-03-03 | c4a23cf | ci: add GitHub Actions workflow for cross-platform release builds
* 2026-03-03 | d60f1dd | ci: include full scripts folder and documentation in release artifacts
* 2026-03-03 | 133f666 | ci: include full scripts folder and documentation in release artifacts
* 2026-03-03 | 38a35a8 | Fix: Static GIF → JXL d=1.0 (was lossless d=0.0, always oversized)
* 2026-03-03 | 3978882 | Fix: Static GIF → JXL d=1.0 (was lossless d=0.0, always oversized)
* 2026-03-03 | 6740dfe | Fix: BMP/ICO/PNM/TGA/HDR/EXR etc. → lossless JXL; complete format_to_string
* 2026-03-03 | 836a30a | Fix: BMP/ICO/PNM/TGA/HDR/EXR etc. → lossless JXL; complete format_to_string
* 2026-03-03 | 82640f3 | ci: fix all platform dependency issues; bump to v0.9.4
* 2026-03-03 | f705f42 | ci: fix all platform dependency issues; bump to v0.9.4
* 2026-03-03 | ffd2b75 | ci: fix dav1d version + macOS x86_64 cross-compile; bump v0.9.5
* 2026-03-03 | 4183ba1 | ci: fix dav1d version + macOS x86_64 cross-compile; bump v0.9.5
* 2026-03-03 | 1ba1d29 | ci: add meson to Linux deps; bump v0.9.6
* 2026-03-03 | cc58cf8 | ci: add meson to Linux deps; bump v0.9.6
* 2026-03-03 | da046c3 | ci: install pkgconfiglite on Windows; bump v0.9.7
* 2026-03-03 | d4f14ce | ci: install pkgconfiglite on Windows; bump v0.9.7
* 2026-03-04 | ad5c0c0 | fix: remove fabricated ExitStatus::default() from fallback pipelines; bump v0.9.8
* 2026-03-04 | 077e0b2 | fix: remove fabricated ExitStatus::default() from fallback pipelines; bump v0.9.8
* 2026-03-04 | 246f6e8 | fix: propagate copy_on_skip_or_fail errors; fix Linux ACL apply to dst
* 2026-03-04 | de48f88 | fix: propagate copy_on_skip_or_fail errors; fix Linux ACL apply to dst
* 2026-03-04 | f654a86 | feat: add Apple Photos library protection
* 2026-03-04 | cb8acfc | feat: add Apple Photos library protection
* 2026-03-05 | 323e165 | fix: detect animated AVIF/JXL/HEIC instead of hardcoding is_animated=false
* 2026-03-05 | 7b1f413 | fix: detect animated AVIF/JXL/HEIC instead of hardcoding is_animated=false
* 2026-03-05 | bc7e21a | fix: deep audit — routing, error propagation, and cjxl precision fixes
* 2026-03-05 | 45dc081 | fix: deep audit — routing, error propagation, and cjxl precision fixes
* 2026-03-05 | 5a11324 | fix: bypass size/quality guard in apple_compat mode for animated image→HEVC
* 2026-03-05 | b4c4e85 | fix: bypass size/quality guard in apple_compat mode for animated image→HEVC
* 2026-03-05 | c1c300e | refactor: unify animated routing to meme-score strategy, remove 4.5s hardcoded threshold
* 2026-03-05 | eaaafad | refactor: unify animated routing to meme-score strategy, remove 4.5s hardcoded threshold
* 2026-03-05 | 3441d33 | fix: fallback to ImageMagick when ffmpeg cannot decode animated WebP for GIF
* 2026-03-05 | 43540fb | fix: fallback to ImageMagick when ffmpeg cannot decode animated WebP for GIF
* 2026-03-05 | 284255b | fix: ImageMagick-first GIF encoding; copy original on all animated conversion failures
* 2026-03-05 | abab8d9 | fix: ImageMagick-first GIF encoding; copy original on all animated conversion failures
* 2026-03-05 | ace403c | feat: iPhone slow-motion VFR handling & fix AA/AEE orphan files
* 2026-03-05 | b4b3ce7 | feat: iPhone slow-motion VFR handling & fix AA/AEE orphan files
* 2026-03-05 | 51275cd | docs: improve VFR detection algorithm for iPhone slow-motion videos
* 2026-03-05 | f003163 | docs: improve VFR detection algorithm for iPhone slow-motion videos
* 2026-03-05 | 81a9253 | Improve VFR detection: use Apple slow-mo tag and frame rate ratio
* 2026-03-05 | 03a6ffd | Improve VFR detection: use Apple slow-mo tag and frame rate ratio
* 2026-03-05 | 88842ef | Improve VFR detection: use Apple slow-mo tag and frame rate ratio
* 2026-03-05 | 7b56dcd | Improve VFR detection: use Apple slow-mo tag and frame rate ratio
* 2026-03-05 | 18ffe5f | release: v0.9.9-3 - Improved VFR detection & AAE file handling
* 2026-03-05 | ede1286 | release: v0.9.9-3 - Improved VFR detection & AAE file handling
* 2026-03-05 | 4f72b28 | Merge branch 'nightly'
* 2026-03-05 | a62db73 | Merge branch 'nightly'
* 2026-03-05 | 5b59ddb | Fix tests: add is_variable_frame_rate field to test cases
* 2026-03-05 | 114bf1c | Fix tests: add is_variable_frame_rate field to test cases
* 2026-03-06 | 9871070 | fix: 临时文件清理、FPS预检查、分辨率修正
* 2026-03-06 | 0151eab | fix: 临时文件清理、FPS预检查、分辨率修正
* 2026-03-08 | 44b0a92 | style: clippy and quality improvements
* 2026-03-08 | 1b7274a | style: clippy and quality improvements
* 2026-03-08 | 9d8bc33 | docs: add MIT license file
* 2026-03-08 | 156019b | docs: add MIT license file
* 2026-03-08 | 4085fdc | chore: update dependencies
* 2026-03-08 | 6f211a9 | chore: update dependencies
* 2026-03-08 | 0e856a5 | chore: upgrade dependencies to latest including incompatible ones
* 2026-03-08 | 357a0fb | chore: upgrade dependencies to latest including incompatible ones
* 2026-03-08 | 1518836 | fix: skip audio demux from image containers in x265 mux step
* 2026-03-08 | e4fb111 | fix: skip audio demux from image containers in x265 mux step
* 2026-03-08 | 146f531 | fix: downgrade NotRecommended precheck from warn to info
* 2026-03-08 | 16562b1 | fix: downgrade NotRecommended precheck from warn to info
* 2026-03-09 | f988a54 | Fix FFmpeg libx265 error for image containers (AVIF/HEIC/GIF/WebP)
* 2026-03-09 | facd993 | Fix FFmpeg libx265 error for image containers (AVIF/HEIC/GIF/WebP)
* 2026-03-09 | 73f519a | chore: bump version to 0.10.1
* 2026-03-09 | c31d179 | chore: bump version to 0.10.1
* 2026-03-09 | b8c74a8 | Update dependencies to nightly versions using git sources
* 2026-03-09 | 723f39c | Update dependencies to nightly versions using git sources
* 2026-03-09 | 7eef34c | Revert to stable dependencies - nightly git sources cause version conflicts
* 2026-03-09 | f990e45 | Revert to stable dependencies - nightly git sources cause version conflicts
* 2026-03-09 | 421763b | v0.10.2: Enhanced meme detection with filename and loop frequency analysis
* 2026-03-09 | 3721fcf | v0.10.2: Enhanced meme detection with filename and loop frequency analysis
* 2026-03-09 | d613c95 | v0.10.3: Fix multi-stream animated files frame loss + preserve FPS
* 2026-03-09 | 625275b | v0.10.3: Fix multi-stream animated files frame loss + preserve FPS
* 2026-03-09 | 5a91f27 | Release v0.10.4: Remove ImageMagick fallback, unify GIF conversion pipeline
* 2026-03-09 | dd0a928 | Release v0.10.4: Remove ImageMagick fallback, unify GIF conversion pipeline
* 2026-03-09 | 551b229 | Release v0.10.5: Add animated JXL support and fix static JXL detection
* 2026-03-09 | 8fee495 | Release v0.10.5: Add animated JXL support and fix static JXL detection
* 2026-03-09 | 9fd0c75 | Fix clippy warnings: code quality improvements
* 2026-03-09 | 22409d0 | Fix clippy warnings: code quality improvements
* 2026-03-09 | 54fea58 | Fix AVIF GBR colorspace bug, WebP dimension detection, and add WebP pre-processing
* 2026-03-09 | 68b3b68 | Fix AVIF GBR colorspace bug, WebP dimension detection, and add WebP pre-processing
* 2026-03-09 | 10be66c | Fix WebP APNG duration detection using FFmpeg
* 2026-03-09 | dc3772d | Fix WebP APNG duration detection using FFmpeg
* 2026-03-09 | f1cf5cc | Fix WebP frame extraction and timing using webpmux
* 2026-03-09 | 9eba15d | Fix WebP frame extraction and timing using webpmux
* 2026-03-09 | a761579 | Fix multi-stream AVIF/HEIC stream selection bug
* 2026-03-09 | add29fa | Fix multi-stream AVIF/HEIC stream selection bug
* 2026-03-09 | 00198e0 | Fix clippy warning: use .find() instead of .skip_while().next()
* 2026-03-09 | dfe1a7c | Fix clippy warning: use .find() instead of .skip_while().next()
* 2026-03-09 | b9a6f88 | Update release workflow to use RELEASE_NOTES file if available
* 2026-03-09 | 203090b | Update release workflow to use RELEASE_NOTES file if available
* 2026-03-09 | aa02499 | Fix ffprobe failures on filenames with special characters ([]{%})
* 2026-03-09 | 616dece | Fix ffprobe failures on filenames with special characters ([]{%})
* 2026-03-09 | 6809bea | Fix misleading quality check messages and improve timestamp verification diagnostics
* 2026-03-09 | 44dd242 | Fix misleading quality check messages and improve timestamp verification diagnostics
* 2026-03-09 | 92d915c | Fix ffprobe image2 demuxer pattern matching and silent errors
* 2026-03-09 | bf9e0be | Fix ffprobe image2 demuxer pattern matching and silent errors
* 2026-03-09 | 3dbbf27 | Change stream_size ffprobe from -v quiet to -v error
* 2026-03-09 | 2c92919 | Change stream_size ffprobe from -v quiet to -v error
* 2026-03-09 | ad69147 | Enhanced size check logging and copy-on-fail feedback
* 2026-03-09 | 76f0467 | Enhanced size check logging and copy-on-fail feedback
* 2026-03-09 | 7a82fac | Changed size tolerance from percentage to KB-level
* 2026-03-09 | 0def125 | Changed size tolerance from percentage to KB-level
* 2026-03-09 | 983e831 | Fixed compress mode to respect tolerance setting
* 2026-03-09 | afa54a2 | Fixed compress mode to respect tolerance setting
* 2026-03-09 | 152114a | feat: Enhanced error logging system with severity levels and auto-classification
* 2026-03-09 | 535f2d9 | feat: Enhanced error logging system with severity levels and auto-classification
* 2026-03-09 | ea0222f | feat: Colorized output, English-only UI, standardized logging macros
* 2026-03-09 | 65d890d | feat: Colorized output, English-only UI, standardized logging macros
* 2026-03-10 | b9fe604 | fix: Colors now render in terminal when launched via drag-drop script or app
* 2026-03-10 | 76549b9 | fix: Colors now render in terminal when launched via drag-drop script or app
* 2026-03-10 | 6b1b780 | v0.10.13: replace [Info] with 📊 emoji on stats lines; add visual separation
* 2026-03-10 | 9eaa3d5 | v0.10.13: replace [Info] with 📊 emoji on stats lines; add visual separation
* 2026-03-10 | e79d953 | v0.10.14: fix all clippy warnings (format! in format! args)
* 2026-03-10 | d4b78ca | v0.10.14: fix all clippy warnings (format! in format! args)
* 2026-03-10 | bcef494 | chore: unify version to v0.10.14 across README and Cargo.toml
* 2026-03-10 | c722d23 | chore: unify version to v0.10.14 across README and Cargo.toml
* 2026-03-10 | 13426d9 | Add compact duration formatting (1d2h3m4s) to progress displays
* 2026-03-10 | 528b96b | Add compact duration formatting (1d2h3m4s) to progress displays
* 2026-03-10 | 9051f36 | Update duration format to detailed style with milliseconds
* 2026-03-10 | f094d06 | Update duration format to detailed style with milliseconds
* 2026-03-10 | adf1f99 | Beautify duration format with elegant standard time notation
* 2026-03-10 | 554ca55 | Beautify duration format with elegant standard time notation
* 2026-03-10 | e127815 | Beautify duration format with proper spacing and normalization
* 2026-03-10 | 6f0319f | Beautify duration format with proper spacing and normalization
* 2026-03-10 | 1215b8c | Beautify duration format with spaces for better readability
* 2026-03-10 | 94d8465 | Beautify duration format with spaces for better readability
* 2026-03-10 | cd898ed | Optimize duration format spacing for better balance
* 2026-03-10 | e1b576d | Optimize duration format spacing for better balance
* 2026-03-10 | 7ec61a8 | Add weeks unit and implement gradual spacing strategy
* 2026-03-10 | f34086f | Add weeks unit and implement gradual spacing strategy
* 2026-03-10 | 1321cfe | Add years and months units with comprehensive time duration support
* 2026-03-10 | 2226277 | Add years and months units with comprehensive time duration support
* 2026-03-10 | 76511b6 | Implement progressive spacing strategy for enhanced visual hierarchy
* 2026-03-10 | b64d4e2 | Implement progressive spacing strategy for enhanced visual hierarchy
* 2026-03-10 | a799f67 | Consolidate redundant log messages for cleaner output
* 2026-03-10 | b92cab2 | Consolidate redundant log messages for cleaner output
* 2026-03-10 | 46388f5 | Restore multi-line log format for better visual presentation
* 2026-03-10 | 3611793 | Restore multi-line log format for better visual presentation
* 2026-03-10 | cd080b8 | Create beautiful single-line log format with visual separators
* 2026-03-10 | 1bc9846 | Create beautiful single-line log format with visual separators
* 2026-03-10 | 4e15d3b | Move single emoji to QUALITY GATE position for better meaning
* 2026-03-10 | 0c1a9cf | Move single emoji to QUALITY GATE position for better meaning
* 2026-03-10 | ff1ee86 | Ensure exactly 4 emojis in both success and failure cases
* 2026-03-10 | 3b0cf69 | Ensure exactly 4 emojis in both success and failure cases
* 2026-03-10 | b9c3141 | Fix emoji logic: use ❌ for failed QUALITY GATE
* 2026-03-10 | ef6bc54 | Fix emoji logic: use ❌ for failed QUALITY GATE
* 2026-03-10 | 104ca45 | Clean up all test-related temporary files
* 2026-03-10 | 9ae8a3f | Clean up all test-related temporary files
* 2026-03-10 | 70ea684 | Update CHANGELOG.md with log beautification improvements
* 2026-03-10 | 292da0f | Update CHANGELOG.md with log beautification improvements
* 2026-03-10 | 8526e4d | Fix terminal running-time residue: remove tee /dev/tty from binary pipeline
* 2026-03-10 | 3d6105e | Fix terminal running-time residue: remove tee /dev/tty from binary pipeline
* 2026-03-10 | 3fe66c1 | Update bash spinner time format to match Rust compact duration format
* 2026-03-10 | b801ae5 | Update bash spinner time format to match Rust compact duration format
* 2026-03-10 | 618f947 | Fix: clear spinner line after processing, restore normal output display
* 2026-03-10 | f4784de | Fix: clear spinner line after processing, restore normal output display
* 2026-03-10 | 426c1b5 | fix: stop spinner before binary runs to prevent terminal line collision
* 2026-03-10 | 2583a30 | fix: stop spinner before binary runs to prevent terminal line collision
* 2026-03-10 | d15784f | Fix: restore Running spinner display during processing
* 2026-03-10 | 9c1b670 | Fix: restore Running spinner display during processing
* 2026-03-10 | effbf83 | Merge branch 'main' into nightly
* 2026-03-10 | a7ef421 | Merge branch 'main' into nightly
* 2026-03-10 | 30a303e | Fix: restore Running spinner display during processing (nightly)
* 2026-03-10 | d66e6ef | Fix: restore Running spinner display during processing (nightly)
* 2026-03-10 | eb0748d | Fix: restore Running spinner display during processing
* 2026-03-10 | 6b8c87a | Fix: restore Running spinner display during processing
* 2026-03-10 | 347e4da | Fix: pause spinner during binary execution, resume after
* 2026-03-10 | ba967bb | Fix: pause spinner during binary execution, resume after
* 2026-03-10 | d491811 | Merge branch 'main' into nightly
* 2026-03-10 | ec0ec92 | Merge branch 'main' into nightly
* 2026-03-10 | 18df035 | Fix: keep spinner visible by capturing binary output silently
* 2026-03-10 | d6c152e | Fix: keep spinner visible by capturing binary output silently
* 2026-03-10 | cd99d55 | Fix: move spinner to terminal title bar to eliminate residue
* 2026-03-10 | d5b91d8 | Fix: move spinner to terminal title bar to eliminate residue
* 2026-03-10 | e06d130 | Simplify title bar spinner: show only ⏱ elapsed time
* 2026-03-10 | ef7e977 | Simplify title bar spinner: show only ⏱ elapsed time
* 2026-03-10 | ab6048c | Sync title bar timer format with Rust format_duration_compact()
* 2026-03-10 | 86620ab | Sync title bar timer format with Rust format_duration_compact()
* 2026-03-10 | 523dcc5 | Increase title bar padding from 30 to 30000 spaces for complete coverage
* 2026-03-10 | 483b407 | Increase title bar padding from 30 to 30000 spaces for complete coverage
* 2026-03-10 | bd136c5 | Combine WALL HIT and Backtrack messages into single line
* 2026-03-10 | 5314916 | Combine WALL HIT and Backtrack messages into single line
* 2026-03-10 | 2e629fc | Improve WALL HIT log format for better readability and aesthetics
* 2026-03-10 | 4355384 | Improve WALL HIT log format for better readability and aesthetics
* 2026-03-10 | 56d186b | Revert to single-line WALL HIT format with emoji at end
* 2026-03-10 | 94702fa | Revert to single-line WALL HIT format with emoji at end
* 2026-03-10 | 4570f31 | Unify emoji placement for all CRF search logs - move to end
* 2026-03-10 | d4ac860 | Unify emoji placement for all CRF search logs - move to end
* 2026-03-10 | ad1251b | Add separators to success cases for unified CRF log format
* 2026-03-10 | 5a08b3e | Add separators to success cases for unified CRF log format
* 2026-03-10 | 5cadf1d | Simplify x265 encoding logs to reduce CLI parameter confusion
* 2026-03-10 | a307567 | Simplify x265 encoding logs to reduce CLI parameter confusion
* 2026-03-10 | d63fbad | Add emoji feedback for x265 encoding steps
* 2026-03-10 | da83460 | Add emoji feedback for x265 encoding steps
* 2026-03-10 | 2c50c91 | Replace 🔥 fire emoji with 🔍 magnifying glass for Ultimate Explore
* 2026-03-10 | 0a43633 | Replace 🔥 fire emoji with 🔍 magnifying glass for Ultimate Explore
* 2026-03-10 | e83015d | Unify per-file log: emoji at tail, fixed-width filename column
* 2026-03-10 | ad8936f | Unify per-file log: emoji at tail, fixed-width filename column
* 2026-03-10 | d383e4e | Unify per-file log: emoji at tail, fixed-width filename column
* 2026-03-10 | babe47b | Unify per-file log: emoji at tail, fixed-width filename column
* 2026-03-10 | eecb118 | Merge branch 'main' into nightly
* 2026-03-10 | 38c4129 | Merge branch 'main' into nightly
* 2026-03-10 | 5e1968b | fix: script syntax error and inconsistent clear-screen on double-click
* 2026-03-10 | 9b9d100 | fix: script syntax error and inconsistent clear-screen on double-click
* 2026-03-10 | 9e372d8 | Merge branch 'main' into nightly
* 2026-03-10 | 7cb6d85 | Merge branch 'main' into nightly
* 2026-03-10 | 384f963 | fix: restore per-file success lines suppressed by quiet mode in batch
* 2026-03-10 | 871a895 | fix: restore per-file success lines suppressed by quiet mode in batch
* 2026-03-10 | d980a1f | Merge branch 'main' into nightly
* 2026-03-10 | ecf66dc | Merge branch 'main' into nightly
* 2026-03-10 | 24ee0ac | fix+feat: raise image decode limit for large JPEGs; add Ctrl+C guard
* 2026-03-10 | d31cbca | fix+feat: raise image decode limit for large JPEGs; add Ctrl+C guard
* 2026-03-10 | d5e801e | Merge branch 'main' into nightly
* 2026-03-10 | 2d42493 | Merge branch 'main' into nightly
* 2026-03-10 | c5cbe90 | fix+feat+refactor: periodic clear fix, emoji prefixes, remove pb/lossless/Simple
* 2026-03-10 | 970779e | fix+feat+refactor: periodic clear fix, emoji prefixes, remove pb/lossless/Simple
* 2026-03-10 | 1536da6 | Merge branch 'main' into nightly
* 2026-03-10 | 141bb51 | Merge branch 'main' into nightly
* 2026-03-10 | 68f6aec | fix: remove leading blank line from milestone status lines to prevent terminal badges
* 2026-03-10 | 08d8db2 | fix: remove leading blank line from milestone status lines to prevent terminal badges
* 2026-03-10 | 1436bda | Merge branch 'main' into nightly
* 2026-03-10 | 254604e | Merge branch 'main' into nightly
* 2026-03-10 | f5470d7 | Release v0.10.19: Update version numbers and documentation
* 2026-03-10 | a39fb44 | Release v0.10.19: Update version numbers and documentation
* 2026-03-10 | 11f97b3 | Fix emoji display issues
* 2026-03-10 | d3bf5a5 | Fix emoji display issues
* 2026-03-10 | 8ffbde8 | Update changelog for emoji bug fixes
* 2026-03-10 | d9d079d | Update changelog for emoji bug fixes
* 2026-03-10 | ea977a5 | fix: script clear-screen, double Ctrl+C, milestone inline display
* 2026-03-10 | 371c3dd | fix: script clear-screen, double Ctrl+C, milestone inline display
* 2026-03-10 | 2299d24 | fix+refactor: compact milestone format, fix title padding leak, Ctrl+C race
* 2026-03-10 | 06d3cc6 | fix+refactor: compact milestone format, fix title padding leak, Ctrl+C race
* 2026-03-10 | 0a27bed | fix: Ctrl+C auto-resume logic, milestone alignment, title padding
* 2026-03-10 | e79d344 | fix: Ctrl+C auto-resume logic, milestone alignment, title padding
* 2026-03-10 | 195c22a | Fix milestone persistent display and implement native Ctrl+C guard
* 2026-03-10 | a76f5ca | Fix milestone persistent display and implement native Ctrl+C guard
* 2026-03-10 | 88635a1 | Fix milestone persistent display and implement native Ctrl+C guard
* 2026-03-10 | 5e3ef69 | Fix milestone persistent display and implement native Ctrl+C guard
* 2026-03-10 | c2f0a09 | Merge main fixes (no version bump)
* 2026-03-10 | 57f51f2 | Merge main fixes (no version bump)
* 2026-03-10 | f674422 | Fix Ctrl+C guard and simplify GIF log format
* 2026-03-10 | 1d97e4a | Fix Ctrl+C guard and simplify GIF log format
* 2026-03-10 | d9ee140 | Fix milestone display after GIF processing logs
* 2026-03-10 | 248af3a | Fix milestone display after GIF processing logs
* 2026-03-10 | 50aa21f | Fix Ctrl+C guard signal handling in pipeline
* 2026-03-10 | 35d5cd5 | Fix Ctrl+C guard signal handling in pipeline
* 2026-03-10 | f446a25 | Systematic fix for Ctrl+C guard signal handling
* 2026-03-10 | aeff861 | Systematic fix for Ctrl+C guard signal handling
* 2026-03-10 | 16253ac | Fix milestone position and GIF log alignment
* 2026-03-10 | e02ad9f | Fix milestone position and GIF log alignment
* 2026-03-10 | f9cc4d8 | 彻底修复 Ctrl+C 守卫信号处理
* 2026-03-10 | 004e293 | 彻底修复 Ctrl+C 守卫信号处理
* 2026-03-10 | 723732f | Clean up all temporary test files
* 2026-03-10 | a895018 | Clean up all temporary test files
* 2026-03-10 | 7d56312 | Remove all shell signal handling - let Rust handle Ctrl+C directly
* 2026-03-10 | 1f62fcc | Remove all shell signal handling - let Rust handle Ctrl+C directly
* 2026-03-10 | bda92b9 | Revert Ctrl+C guard to original working version
* 2026-03-10 | b72db32 | Revert Ctrl+C guard to original working version
* 2026-03-10 | 0c5a45f | Restore log display fixes from previous attempts
* 2026-03-10 | bf54592 | Restore log display fixes from previous attempts
* 2026-03-10 | 6becb36 | Fix conversion message to use correct English term 'transcoding'
* 2026-03-10 | 2834878 | Fix conversion message to use correct English term 'transcoding'
* 2026-03-10 | 4e93876 | Remove redundant 'successful' text since ✅ emoji already indicates success
* 2026-03-10 | df168d7 | Remove redundant 'successful' text since ✅ emoji already indicates success
* 2026-03-10 | 1c3e719 | Change GIF text to 'Animation' in English
* 2026-03-10 | 2ae1fa5 | Change GIF text to 'Animation' in English
* 2026-03-10 | 7677996 | Fix conversion message to prevent truncation
* 2026-03-10 | 7055d24 | Fix conversion message to prevent truncation
* 2026-03-10 | a75c8be | feat: modernize log format, fix terminal colors, rewrite ctrl+c guard, audit & update deps
* 2026-03-10 | af1c13f | feat: modernize log format, fix terminal colors, rewrite ctrl+c guard, audit & update deps
* 2026-03-11 | 37f7d52 | fix: make bash script compatible with Rust interactive features
* 2026-03-11 | 09a2de0 | fix: make bash script compatible with Rust interactive features
* 2026-03-11 | 1738142 | fix: robust SIGINT pipeline handling and inline terminal stats
* 2026-03-11 | 5042a8b | fix: robust SIGINT pipeline handling and inline terminal stats
* 2026-03-11 | bfc4fa7 | fix: restore ANSI colors stripped by refactoring, remove unused TTY code, and consolidate changelog
* 2026-03-11 | b13975e | fix: restore ANSI colors stripped by refactoring, remove unused TTY code, and consolidate changelog
* 2026-03-11 | de2dd33 | fix: correctly terminate background title spinner on pipeline Ctrl+C interruptions
* 2026-03-11 | 1e8930f | fix: correctly terminate background title spinner on pipeline Ctrl+C interruptions
* 2026-03-11 | 1da93c6 | fix(ui & termination): ensure colors render and subprocesses quit reliably on Ctrl+C
* 2026-03-11 | c539664 | fix(ui & termination): ensure colors render and subprocesses quit reliably on Ctrl+C
* 2026-03-11 | bd654f1 | fix: enforce thread suspension on Ctrl+C prompt & overhaul terminal UI aesthetics
* 2026-03-11 | 38a974b | fix: enforce thread suspension on Ctrl+C prompt & overhaul terminal UI aesthetics
* 2026-03-11 | 599e6d8 | Merge branch 'main' into nightly
* 2026-03-11 | fae1291 | Merge branch 'main' into nightly
* 2026-03-11 | 571ef44 | chore: Standardized 1MB file size limits and translated Simplified Chinese internal outputs
* 2026-03-11 | 68bfc51 | chore: Standardized 1MB file size limits and translated Simplified Chinese internal outputs
* 2026-03-11 | 44d9a5e | chore: updated dependencies and translated remaining test assertions to English
* 2026-03-11 | d3bf728 | chore: updated dependencies and translated remaining test assertions to English
* 2026-03-11 | 1ac6445 | chore: make can_compress_pure_video respect allow_size_tolerance flag
* 2026-03-11 | 38d82eb | chore: make can_compress_pure_video respect allow_size_tolerance flag
* 2026-03-11 | 5fba6d0 | feat: implement precision-first quality detection for video (CRF/B-frames) and images (HEIC/AVIF/TIFF/JXL/JP2)
* 2026-03-11 | 7a78102 | feat: implement precision-first quality detection for video (CRF/B-frames) and images (HEIC/AVIF/TIFF/JXL/JP2)
* 2026-03-11 | 453c6e0 | feat: implement precision-first quality detection across all formats and fix workspace build errors
* 2026-03-11 | 1103319 | feat: implement precision-first quality detection across all formats and fix workspace build errors
* 2026-03-11 | fc8fb08 | chore: fix clippy  warning in image_detection.rs
* 2026-03-11 | 9d98595 | chore: fix clippy  warning in image_detection.rs
* 2026-03-11 | 019703d | feat: use precision-first strategy for image quality detection
* 2026-03-11 | 7ad8356 | feat: use precision-first strategy for image quality detection
* 2026-03-11 | e936162 | feat(av1): sync AV1 animated image encoding with HEVC parity
* 2026-03-11 | 1d74242 | feat(av1): sync AV1 animated image encoding with HEVC parity
* 2026-03-11 | 4041891 | chore: release v0.10.26 - Precision-first metadata, Ultimate Wall Detection, and UI Overhaul
* 2026-03-11 | 72492af | chore: release v0.10.26 - Precision-first metadata, Ultimate Wall Detection, and UI Overhaul
* 2026-03-12 | 495e257 | feat: implement quality fast-fail in upward search and increase saturation to 30 for Ultimate Mode
* 2026-03-12 | 8aa7360 | feat: implement quality fast-fail in upward search and increase saturation to 30 for Ultimate Mode
* 2026-03-12 | ceaaa05 | feat: increase saturation to 30 and add 3-sample confirmation for quality fast-fail
* 2026-03-12 | daf5e0b | feat: increase saturation to 30 and add 3-sample confirmation for quality fast-fail
* 2026-03-12 | 4fbcf0f | feat: implement 10-step confirmation window for Ultimate wall detection to avoid noise-induced early exit
* 2026-03-12 | 76bc97d | feat: implement 10-step confirmation window for Ultimate wall detection to avoid noise-induced early exit
* 2026-03-12 | 2675e46 | feat: implement 'Dead-Wall' fast-fail in downward search to prevent performance waste on non-recoverable quality
* 2026-03-12 | a7ddbcf | feat: implement 'Dead-Wall' fast-fail in downward search to prevent performance waste on non-recoverable quality
* 2026-03-12 | c9f5675 | feat: implement sticky quality insights and 50-step extreme saturation for Ultimate Mode
* 2026-03-12 | 8163294 | feat: implement sticky quality insights and 50-step extreme saturation for Ultimate Mode
* 2026-03-12 | 121fdc0 | fix: prevent early termination in Ultimate Mode when hitting standard min_crf boundary
* 2026-03-12 | 6b80f94 | fix: prevent early termination in Ultimate Mode when hitting standard min_crf boundary
* 2026-03-12 | 45d7b18 | feat: remove CRF floor in Ultimate Mode to allow hitting true physical walls at any CRF
* 2026-03-12 | 0f626fc | feat: remove CRF floor in Ultimate Mode to allow hitting true physical walls at any CRF
* 2026-03-12 | 5f104b4 | feat: accelerated CPU fine-tuning with Sprint & Backtrack and removed CRF barriers
* 2026-03-12 | 46a6656 | feat: accelerated CPU fine-tuning with Sprint & Backtrack and removed CRF barriers
* 2026-03-12 | df7be3b | feat: unified 10-sample integer quality insight mechanism across all phases (v0.10.34)
* 2026-03-12 | 3560cd0 | feat: unified 10-sample integer quality insight mechanism across all phases (v0.10.34)
* 2026-03-12 | 609cf4d | feat: optimize quality insight mechanism and 1MB tolerance logic (v0.10.35)
* 2026-03-12 | 60af964 | feat: optimize quality insight mechanism and 1MB tolerance logic (v0.10.35)
* 2026-03-12 | 7c30e4e | feat: Add sprint and backtrack mechanism in CPU 0.1 fine-tuning phase
* 2026-03-12 | 1df988d | feat: Add sprint and backtrack mechanism in CPU 0.1 fine-tuning phase
* 2026-03-12 | cf1ecae | feat: restore 453c6e0 precision detection + hardware-aware logging [GPU/CPU]
* 2026-03-12 | dfdc51e | feat: restore 1103319 precision detection + hardware-aware logging [GPU/CPU]
* 2026-03-12 | 460e9ff | feat: enhance GPU/CPU phase distinction in logs & clean up fake fallbacks
* 2026-03-12 | 919a39f | feat: enhance GPU/CPU phase distinction in logs & clean up fake fallbacks
* 2026-03-13 | 0b10ad5 | feat: unified error handling, enhanced logging & algorithm optimizations
* 2026-03-13 | 6b9e614 | feat: unified error handling, enhanced logging & algorithm optimizations
* 2026-03-13 | 1f3499f | test: update test expectations for new constants
* 2026-03-13 | 95c052d | test: update test expectations for new constants
* 2026-03-13 | 8411fb1 | docs: update CHANGELOG for v0.10.36
* 2026-03-13 | e2d3da1 | docs: update CHANGELOG for v0.10.36
* 2026-03-13 | bdc2a9f | Merge nightly into main - v0.10.36
* 2026-03-13 | c7e57bb | Merge nightly into main - v0.10.36
* 2026-03-13 | e27c53a | feat: unified error handling, test fixes, and code cleanup (v0.10.37)
* 2026-03-13 | 885b618 | feat: unified error handling, test fixes, and code cleanup (v0.10.37)
* 2026-03-13 | 7add822 | chore: remove unused progress modules
* 2026-03-13 | 9b65892 | chore: remove unused progress modules
* 2026-03-13 | 20e91da | fix: remove silent CRF defaults and fix Phase 2 algorithm issues
* 2026-03-13 | 6b9ccdf | fix: remove silent CRF defaults and fix Phase 2 algorithm issues
* 2026-03-13 | 7c551f3 | fix(Phase 1): add VMAF/PSNR-UV early insight with integer-level improvement detection
* 2026-03-13 | 2c75aa7 | fix(Phase 1): add VMAF/PSNR-UV early insight with integer-level improvement detection
* 2026-03-13 | cc524ae | fix(Phase 4): skip 0.01-granularity when early insight triggered
* 2026-03-13 | 3bf454b | fix(Phase 4): skip 0.01-granularity when early insight triggered
* 2026-03-13 | 4706a16 | feat: skip quality verification when early insight triggered
* 2026-03-13 | 2f1a6f9 | feat: skip quality verification when early insight triggered
* 2026-03-13 | 3a703fa | fix: early insight only triggers when quality meets thresholds
* 2026-03-13 | daca562 | fix: early insight only triggers when quality meets thresholds
* 2026-03-13 | 0591a9e | Fix early insight logic and CRF 40 fallback in GPU coarse search
* 2026-03-13 | b5ed27f | Fix early insight logic and CRF 40 fallback in GPU coarse search
* 2026-03-13 | 62fe5e0 | Improve Phase 3 efficiency and GPU precision
* 2026-03-13 | da2eb99 | Improve Phase 3 efficiency and GPU precision
* 2026-03-13 | 0b0115c | fix: Phase 2/3 algorithm bugs and logging improvements
* 2026-03-13 | c54f6c6 | fix: Phase 2/3 algorithm bugs and logging improvements
* 2026-03-13 | 98d5690 | fix: add quality metrics to early insight log
* 2026-03-13 | 8d95dfd | fix: add quality metrics to early insight log
* 2026-03-13 | e1927a8 | feat: increase GPU utilization in ultimate mode with precise exploration
* 2026-03-13 | e65396e | feat: increase GPU utilization in ultimate mode with precise exploration
* 2026-03-13 | d8fa914 | fix: enable GPU exploration for small files in ultimate mode
* 2026-03-13 | f9ad142 | fix: enable GPU exploration for small files in ultimate mode
* 2026-03-13 | caa499c | fix: adjust GPU skip threshold to prevent hang on tiny files
* 2026-03-13 | 65898db | fix: adjust GPU skip threshold to prevent hang on tiny files
* 2026-03-13 | 99262b4 | fix: use integer GPU step sizes to prevent hang, increase iterations
* 2026-03-13 | 8c97f3c | fix: use integer GPU step sizes to prevent hang, increase iterations
* 2026-03-13 | ed9b329 | fix: reduce GPU sample duration to prevent timeout hang
* 2026-03-13 | bc0852e | fix: reduce GPU sample duration to prevent timeout hang
* 2026-03-13 | 09fafd5 | feat: restore 0.5-0.1 GPU steps and lower Stage 1 threshold
* 2026-03-13 | 29f2cac | feat: restore 0.5-0.1 GPU steps and lower Stage 1 threshold
* 2026-03-13 | d695891 | fix: enable GPU search logs in ultimate mode for transparency
* 2026-03-13 | 35cbc5a | fix: enable GPU search logs in ultimate mode for transparency
* 2026-03-13 | b931cd7 | feat: enhance temp file security with unique IDs and update dependencies to v0.10.37
* 2026-03-13 | d020474 | feat: enhance temp file security with unique IDs and update dependencies to v0.10.37
* 2026-03-13 | 5f5545d | feat: increase GPU and CPU sampling durations in ultimate mode by 15s
* 2026-03-13 | aab1925 | feat: increase GPU and CPU sampling durations in ultimate mode by 15s
* 2026-03-13 | 0458b5e | chore: release 0.10.38 - Fix temp file cleanup, PSNR calc, and container overhead
* 2026-03-13 | 11e53d5 | chore: release 0.10.38 - Fix temp file cleanup, PSNR calc, and container overhead
* 2026-03-13 | 2db3b45 | feat(gpu_search): Optimize GPU search efficiency for low bitrate videos (<5Mbps)
* 2026-03-13 | d5e8462 | feat(gpu_search): Optimize GPU search efficiency for low bitrate videos (<5Mbps)
* 2026-03-13 | 62f61b4 | feat: add image quality metrics to logs and bump version to v0.10.39
* 2026-03-13 | c38fe0b | feat: add image quality metrics to logs and bump version to v0.10.39
* 2026-03-13 | 6b4e1e7 | feat: implement JSON-based extensible image classification rule engine and expansion
* 2026-03-13 | af72fdc | feat: implement JSON-based extensible image classification rule engine and expansion
* 2026-03-13 | 9c93df7 | feat: hide JPEG transcoding logs from terminal by default (always in log file)
* 2026-03-13 | 555a912 | feat: hide JPEG transcoding logs from terminal by default (always in log file)
* 2026-03-13 | 7bea44a | feat: unified milestone statistics and enhanced log alignment
* 2026-03-13 | 6e4966b | feat: unified milestone statistics and enhanced log alignment
* 2026-03-13 | eed05ce | feat: add MANGA category and refine DOCUMENT classification rules
* 2026-03-13 | f91e9c8 | feat: add MANGA category and refine DOCUMENT classification rules
* 2026-03-13 | f5fbab7 | feat: remove format recommendation from image_classifiers.json
* 2026-03-13 | f4309f2 | feat: remove format recommendation from image_classifiers.json
* 2026-03-13 | 4b3cdb9 | feat: Full Logging System Overhaul with Premium Aesthetics
* 2026-03-13 | 8d4f68c | feat: Full Logging System Overhaul with Premium Aesthetics
* 2026-03-14 | 6be689f | fix: Resolve duplicate milestone stats and clean up multi-line logs
* 2026-03-14 | 03eaac0 | fix: Resolve duplicate milestone stats and clean up multi-line logs
* 2026-03-14 | e707293 | feat: Minimalist Abbreviated Milestones for Video Mode
* 2026-03-14 | f33040d | feat: Minimalist Abbreviated Milestones for Video Mode
* 2026-03-14 | f523fe8 | feat: Add XMP shorthand (X:) support to Video Mode milestones
* 2026-03-14 | 7a17a07 | feat: Add XMP shorthand (X:) support to Video Mode milestones
* 2026-03-14 | 2f0be3e | feat: release v0.10.43
* 2026-03-14 | 2f9c613 | feat: release v0.10.43
* 2026-03-14 | 7108eb3 | fix: eliminate hardcoded quality degradation in image routing
* 2026-03-14 | 0dd5558 | fix: eliminate hardcoded quality degradation in image routing
* 2026-03-14 | fd3bb98 | fix: refine image quality routing and update startup logs
* 2026-03-14 | b3ce399 | fix: refine image quality routing and update startup logs
* 2026-03-14 | c2bb88d | fix: suppress deprecation warnings in routing logic
* 2026-03-14 | dbb36be | fix: suppress deprecation warnings in routing logic
* 2026-03-14 | c17d689 | chore: release v0.10.45
* 2026-03-14 | 51008fa | chore: release v0.10.45
* 2026-03-14 | 59cc4d0 | feat: lossless routing for WebP/AVIF/TIFF → JXL; exclude HEIC/HEIF
* 2026-03-14 | 6e80913 | feat: lossless routing for WebP/AVIF/TIFF → JXL; exclude HEIC/HEIF
* 2026-03-14 | 568c81b | chore: release v0.10.46 with enhanced modern-lossy-skip and heuristic fix
* 2026-03-14 | 09d3188 | chore: release v0.10.46 with enhanced modern-lossy-skip and heuristic fix
* 2026-03-14 | 98ecd4d | feat: add lossless HEIC/HEIF to JXL conversion route
* 2026-03-14 | 117d3fc | feat: add lossless HEIC/HEIF to JXL conversion route
* 2026-03-14 | b2823ca | fix: correct HEIC/HEIF skip logic to match WebP/AVIF pattern
* 2026-03-14 | 6ceb6f5 | fix: correct HEIC/HEIF skip logic to match WebP/AVIF pattern
* 2026-03-14 | 5ac8656 | Add HEVC transquant_bypass detection and mp4parse dependency
* 2026-03-14 | ff88e12 | Add HEVC transquant_bypass detection and mp4parse dependency
* 2026-03-14 | 390edec | fix: restore safe fallback behavior for corrupted media files
* 2026-03-14 | 806a5c9 | fix: restore safe fallback behavior for corrupted media files
* 2026-03-14 | a6e129d | fix: silence cache debug logs and prevent stack overflow
* 2026-03-14 | 2265c2a | fix: silence cache debug logs and prevent stack overflow
* 2026-03-14 | 5c850d4 | feat: enrich analysis cache and fix UI labels
* 2026-03-14 | 6e20956 | feat: enrich analysis cache and fix UI labels
* 2026-03-14 | 0cee0e8 | release: v0.10.49 - README overhaul and HEIC security fix
* 2026-03-14 | 3b49b5b | release: v0.10.49 - README overhaul and HEIC security fix
* 2026-03-14 | 06fced5 | feat: explicit size units in logs (v0.10.50)
* 2026-03-14 | 188f0b2 | feat: explicit size units in logs (v0.10.50)
* 2026-03-14 | fbc6c96 | refactor: remove dynamic compression adjustment and legacy routing (v0.10.51)
* 2026-03-14 | 54cdcd6 | refactor: remove dynamic compression adjustment and legacy routing (v0.10.51)
* 2026-03-14 | 288831c | fix: simplify image classifiers usage and log all fallbacks
* 2026-03-14 | 765f374 | fix: simplify image classifiers usage and log all fallbacks
* 2026-03-14 | 2f9505b | tune: refine gif meme-score heuristics for tiny stickers
* 2026-03-14 | bb697a0 | tune: refine gif meme-score heuristics for tiny stickers
* 2026-03-14 | 3550944 | tune: sharpen gif meme-score for stickers and social-cache names
* 2026-03-14 | 8b87107 | tune: sharpen gif meme-score for stickers and social-cache names
* 2026-03-15 | 1ba9ab8 | chore: bump version to 0.10.52 and perfected meme scoring mechanism
* 2026-03-15 | 3384e4c | chore: bump version to 0.10.52 and perfected meme scoring mechanism
* 2026-03-15 | 8467207 | fix: resolve GIF parser desync and implement performance-optimized Joint Audit
* 2026-03-15 | 731ee96 | fix: resolve GIF parser desync and implement performance-optimized Joint Audit
* 2026-03-15 | 543e198 | feat: implement 3-stage cross-audit with deep byte-level bitstream investigation
* 2026-03-15 | e035471 | feat: implement 3-stage cross-audit with deep byte-level bitstream investigation
* 2026-03-15 | 6c3bfea | fix: resolve compilation errors and implement internal deep byte-research for joint audit
* 2026-03-15 | cd49c08 | fix: resolve compilation errors and implement internal deep byte-research for joint audit
* 2026-03-15 | 1a7cfa0 | feat: implement robust persistent cache with nanosecond change detection and SQL migration
* 2026-03-15 | 021e740 | feat: implement robust persistent cache with nanosecond change detection and SQL migration
* 2026-03-15 | 71c092a | feat: implement Video CRF search hint (warm start) v0.10.57
* 2026-03-15 | 2a5cd19 | feat: implement Video CRF search hint (warm start) v0.10.57
* 2026-03-15 | 1b58859 | chore: update gitignore for local caches and tool configs
* 2026-03-15 | 23e65a6 | chore: update gitignore for local caches and tool configs
* 2026-03-15 | 54c20b7 | feat: implement global CRF warm start cache for video and dynamic images
* 2026-03-15 | a8d3aac | feat: implement global CRF warm start cache for video and dynamic images
* 2026-03-15 | 34908b1 | fix: unnecessary parentheses around assigned value
* 2026-03-15 | 42cb077 | fix: unnecessary parentheses around assigned value
* 2026-03-15 | 208c468 | feat: enhance detect_animation with ffprobe/libavformat fallback
* 2026-03-15 | b9c8433 | feat: enhance detect_animation with ffprobe/libavformat fallback
* 2026-03-15 | 564e81c | refactor(animation): fully trust ffprobe for ISOBMFF formats like AVIF to avoid false positives
* 2026-03-15 | dff899c | refactor(animation): fully trust ffprobe for ISOBMFF formats like AVIF to avoid false positives
* 2026-03-15 | 9d07b93 | fix(heic): remove extension fallback from format detection to prevent NoFtypBox false errors
* 2026-03-15 | ef4ca89 | fix(heic): remove extension fallback from format detection to prevent NoFtypBox false errors
* 2026-03-15 | f109e9b | fix(heic): use numeric value for LIBHEIF_SECURITY_LIMITS to prevent NoFtypBox error
* 2026-03-15 | 860c9ca | fix(heic): use numeric value for LIBHEIF_SECURITY_LIMITS to prevent NoFtypBox error
* 2026-03-15 | a6427eb | fix(heic): add robust fallback to read_from_file and verify security limits
* 2026-03-15 | 4494c8f | fix(heic): add robust fallback to read_from_file and verify security limits
* 2026-03-15 | 6e38294 | fix(heic): complete brand list (heix, hevc, hevx) and add diagnostic tag V3
* 2026-03-15 | 26c3c2b | fix(heic): complete brand list (heix, hevc, hevx) and add diagnostic tag V3
* 2026-03-15 | c71f930 | refactor(heic): rename to analyze_heic_file_v4 and add V4 diagnostic tags
* 2026-03-15 | 4d8f97d | refactor(heic): rename to analyze_heic_file_v4 and add V4 diagnostic tags
* 2026-03-15 | 7dc4092 | fix(heic): final V4 cleanup, remove panic and restore security limits
* 2026-03-15 | bb404c6 | fix(heic): final V4 cleanup, remove panic and restore security limits
* 2026-03-15 | 6212773 | fix(heic): set LIBHEIF_SECURITY_LIMITS at global program entry points
* 2026-03-15 | 17ec85d | fix(heic): set LIBHEIF_SECURITY_LIMITS at global program entry points
* 2026-03-15 | cfc083b | v0.10.59: Cache version control + HEIC lossless detection fix
* 2026-03-15 | a2b3d34 | v0.10.59: Cache version control + HEIC lossless detection fix
* 2026-03-15 | 72d374f | v0.10.60: Log level optimization + dependency updates
* 2026-03-15 | 5f8bf26 | v0.10.60: Log level optimization + dependency updates
* 2026-03-15 | fd8fb02 | v0.10.61: Bind cache version to program version for automatic invalidation
* 2026-03-15 | c668aff | v0.10.61: Bind cache version to program version for automatic invalidation
* 2026-03-15 | 73cb2cf | Add WebP/AVIF lossless detection verification
* 2026-03-15 | 5e4447f | Add WebP/AVIF lossless detection verification
* 2026-03-15 | 85fd073 | v0.10.62: Unify dependencies to GitHub nightly sources
* 2026-03-15 | 4d07d80 | v0.10.62: Unify dependencies to GitHub nightly sources
* 2026-03-15 | 4ce3ddf | Fix compilation warning in nightly branch
* 2026-03-15 | 9d1ee4c | Fix compilation warning in nightly branch
* 2026-03-15 | a2eff7f | v0.10.63: Increase HEIC security limits
* 2026-03-15 | dcf8fd8 | v0.10.63: Increase HEIC security limits
* 2026-03-15 | e025fc1 | Remove AI tool config folders from Git tracking
* 2026-03-15 | 193feb8 | Remove AI tool config folders from Git tracking
* 2026-03-15 | 6bf75cc | fix: remove .clippy.toml from .gitignore (should be tracked)
* 2026-03-15 | 4900f5e | fix: remove .clippy.toml from .gitignore (should be tracked)
* 2026-03-15 | 283b936 | chore: bump version to 0.10.64
* 2026-03-15 | 9a547f5 | chore: bump version to 0.10.64
* 2026-03-15 | 7511d2b | ci: restore release workflow and add v0.10.64 release notes
* 2026-03-15 | 8216e56 | ci: restore release workflow and add v0.10.64 release notes
* 2026-03-15 | befef0e | fix: apply HEIC security limits before reading file (v0.10.65)
* 2026-03-15 | 240f0e5 | fix: apply HEIC security limits before reading file (v0.10.65)
* 2026-03-15 | e9a6ad7 | fix: remove LIBHEIF_SECURITY_LIMITS env var, use API-level limits only
* 2026-03-15 | e6f0e20 | fix: remove LIBHEIF_SECURITY_LIMITS env var, use API-level limits only
* 2026-03-16 | 4f8a9ca | fix: enable v1_21 feature in img_hevc/img_av1 + increase HEIC limits to 15GB (v0.10.66)
* 2026-03-16 | 863900c | fix: enable v1_21 feature in img_hevc/img_av1 + increase HEIC limits to 15GB (v0.10.66)
* 2026-03-16 | c12429f | fix: enable v1_21 in shared_utils default feature (critical fix)
* 2026-03-16 | 56b422d | fix: enable v1_21 in shared_utils default feature (critical fix)
* 2026-03-16 | 55bda88 | fix: correct HEIC security limits API usage + restore fallback 2 (v0.10.66)
* 2026-03-16 | a34ee26 | fix: correct HEIC security limits API usage + restore fallback 2 (v0.10.66)
* 2026-03-16 | d3f02f9 | fix: clippy warnings - simplify logic and add allow attributes
* 2026-03-16 | 702fcac | fix: clippy warnings - simplify logic and add allow attributes
* 2026-03-16 | 2a2a99c | fix: resolve all clippy warnings in workspace
* 2026-03-16 | daaef9d | fix: resolve all clippy warnings in workspace
* 2026-03-16 | 2aa3a0f | fix: preserve file creation time and clean log output (v0.10.67)
* 2026-03-16 | fefbcee | fix: preserve file creation time and clean log output (v0.10.67)
* 2026-03-16 | a252364 | fix: comprehensive metadata preservation across all platforms (v0.10.68)
* 2026-03-16 | cd37370 | fix: comprehensive metadata preservation across all platforms (v0.10.68)
* 2026-03-16 | c91553f | fix: enable metadata preservation by default (v0.10.69)
* 2026-03-16 | 69f182f | fix: enable metadata preservation by default (v0.10.69)
* 2026-03-16 | 7cdc22e | feat(cache): Enhanced cache system v3 with content fingerprint and integrity verification
* 2026-03-16 | f6ff095 | feat(cache): Enhanced cache system v3 with content fingerprint and integrity verification
* 2026-03-16 | 5a55b24 | docs: clarify nightly-only GitHub dependencies in Cargo.toml
* 2026-03-16 | 393d72e | docs: clarify nightly-only GitHub dependencies in Cargo.toml
* 2026-03-16 | 5acc824 | feat: nightly branch uses GitHub dependencies for latest iterations
* 2026-03-16 | e5e1018 | feat: nightly branch uses GitHub dependencies for latest iterations
* 2026-03-16 | 48c222c | feat: main branch uses stable crates.io dependencies
* 2026-03-16 | 82be7bf | feat: main branch uses stable crates.io dependencies
* 2026-03-16 | 815772b | feat: unified version management system
* 2026-03-16 | cdbb7b8 | feat: unified version management system
* 2026-03-16 | fe23a4f | feat: unified version management system
* 2026-03-16 | 65a2ada | feat: unified version management system
* 2026-03-16 | d08485d | v0.10.72: Fix ICC Profile & Metadata Preservation
* 2026-03-16 | 4f9ed02 | v0.10.72: Fix ICC Profile & Metadata Preservation
* 2026-03-16 | f6ce3de | v0.10.71: Complete metadata preservation fix
* 2026-03-16 | 9c422a9 | v0.10.71: Complete metadata preservation fix
* 2026-03-16 | 5eca0a6 | nightly: Restore GitHub dependencies for latest iterations
* 2026-03-16 | db71ab1 | nightly: Restore GitHub dependencies for latest iterations
* 2026-03-19 | 31e5fe2 | v0.10.73: Compilation warnings fixed and unified version management
* 2026-03-19 | 55bf6ad | v0.10.73: Compilation warnings fixed and unified version management
* 2026-03-19 | 9be8d70 | main: Restore crates.io dependencies for stable production use
* 2026-03-19 | 93fd794 | main: Restore crates.io dependencies for stable production use
* 2026-03-19 | e8a60cb | feat: Add disk space pre-check to img-hevc
* 2026-03-19 | c46d459 | feat: Add disk space pre-check to img-hevc
* 2026-03-19 | 394afe4 | fix: Script menu flow and disk space pre-check integration
* 2026-03-19 | ea9f3b3 | fix: Script menu flow and disk space pre-check integration
* 2026-03-19 | cd58b8c | nightly: Restore GitHub dependencies for latest iterations
* 2026-03-19 | d7d5bad | nightly: Restore GitHub dependencies for latest iterations
* 2026-03-19 | dbd4c27 | v0.10.74: PNG quantization heuristic accuracy overhaul
* 2026-03-19 | ebb91d3 | v0.10.74: PNG quantization heuristic accuracy overhaul
* 2026-03-19 | f277917 | v0.10.74: PNG quantization heuristic accuracy overhaul
* 2026-03-19 | 7cbe56d | v0.10.74: PNG quantization heuristic accuracy overhaul
* 2026-03-19 | 97c73cb | v0.10.75: Fix stride bias in color frequency distribution sampling
* 2026-03-19 | 4614097 | v0.10.75: Fix stride bias in color frequency distribution sampling
* 2026-03-19 | eb16680 | v0.10.75: Fix stride bias in color frequency distribution sampling
* 2026-03-19 | b6c8bd4 | v0.10.75: Fix stride bias in color frequency distribution sampling
* 2026-03-20 | ff9748c | v0.10.76: Fix VMAF/SSIM/PSNR filter graph -22 EINVAL on odd-dimension video
* 2026-03-20 | 1059bd8 | v0.10.76: Fix VMAF/SSIM/PSNR filter graph -22 EINVAL on odd-dimension video
* 2026-03-20 | ddba28d | v0.10.76: Fix VMAF/SSIM/PSNR filter graph -22 EINVAL on odd-dimension video
* 2026-03-20 | 4d688a2 | v0.10.76: Fix VMAF/SSIM/PSNR filter graph -22 EINVAL on odd-dimension video
* 2026-03-20 | c10e434 | feat: level up AV1 tools maturity to parity with HEVC, implement CacheStats and GIF meme-score config parity; add GitHub workflow for nightly releases
* 2026-03-20 | b4a2671 | feat: level up AV1 tools maturity to parity with HEVC, implement CacheStats and GIF meme-score config parity; add GitHub workflow for nightly releases
* 2026-03-20 | 0e1d51b | Merge branch 'main' into nightly
* 2026-03-20 | 0213b6b | Merge branch 'main' into nightly
* 2026-03-20 | b761879 | feat: complete av1 tools parity with hevc tools (small png optimization & finalize logic)
* 2026-03-20 | ad1955f | feat: complete av1 tools parity with hevc tools (small png optimization & finalize logic)
* 2026-03-20 | f06a628 | chore: restore clean crates.io dependencies for main branch
* 2026-03-20 | d48d9ea | chore: restore clean crates.io dependencies for main branch
* 2026-03-20 | 8866d56 | chore: bump version to v0.10.78 and update docs
* 2026-03-20 | 8939374 | chore: bump version to v0.10.78 and update docs
* 2026-03-20 | dabe39b | chore: bump version to v0.10.78 and update docs
* 2026-03-20 | 3691046 | chore: bump version to v0.10.78 and update docs
* 2026-03-20 | 6f61fc8 | Merge branch 'nightly'
* 2026-03-20 | 9bc4058 | Merge branch 'nightly'
* 2026-03-20 | 0a22e6c | chore: stabilize main branch by removing git dependencies and fixing version regressions
* 2026-03-20 | fe0cf5f | chore: stabilize main branch by removing git dependencies and fixing version regressions
* 2026-03-20 | d3235ed | chore: fix clippy warnings
* 2026-03-20 | 20f273e | chore: fix clippy warnings
* 2026-03-20 | e436212 | Merge branch 'nightly'
* 2026-03-20 | dc0478f | Merge branch 'nightly'
* 2026-03-20 | b80f68b | Fix hardcoded JXL confidence and progress loading
* 2026-03-20 | 72e546e | Fix hardcoded JXL confidence and progress loading
* 2026-03-20 | 4ca71da | Fix hardcoded JXL confidence and progress loading
* 2026-03-20 | 482e8b2 | Fix hardcoded JXL confidence and progress loading
* 2026-03-20 | 520498e | Fix MS-SSIM resize chain on main deps
* 2026-03-20 | 43829f9 | Fix MS-SSIM resize chain on main deps
* 2026-03-20 | ee04d6c | Fix MS-SSIM resize chain on main deps
* 2026-03-20 | f745da3 | Fix MS-SSIM resize chain on main deps
* 2026-03-20 | 695734a | Make MS-SSIM resize portable across image deps
* 2026-03-20 | bf89ec2 | Make MS-SSIM resize portable across image deps
* 2026-03-20 | 95e99ee | Make MS-SSIM resize portable across image deps
* 2026-03-20 | 1edcd62 | Make MS-SSIM resize portable across image deps
* 2026-03-20 | 366780c | Remove hardcoded Q85 lossy fallback
* 2026-03-20 | 9aeab12 | Remove hardcoded Q85 lossy fallback
* 2026-03-20 | 793da37 | Remove hardcoded Q85 lossy fallback
* 2026-03-20 | 6f866b6 | Remove hardcoded Q85 lossy fallback
* 2026-03-20 | 7c8aa4e | Make thread allocation react to multi-instance mode
* 2026-03-20 | 24f586b | Make thread allocation react to multi-instance mode
* 2026-03-20 | c7a5f6a | Make thread allocation react to multi-instance mode
* 2026-03-20 | bd28ec3 | Make thread allocation react to multi-instance mode
* 2026-03-20 | 9c22bb8 | Relax path validation for argv-safe paths
* 2026-03-20 | 6e495df | Relax path validation for argv-safe paths
* 2026-03-20 | 3cc35f9 | Relax path validation for argv-safe paths
* 2026-03-20 | 6c4c4cf | Relax path validation for argv-safe paths
* 2026-03-20 | dba31ee | Harden app and drag-drop shell entrypoints
* 2026-03-20 | 763de29 | Harden app and drag-drop shell entrypoints
* 2026-03-20 | c5f2ef7 | Harden app and drag-drop shell entrypoints
* 2026-03-20 | a1b0143 | Harden app and drag-drop shell entrypoints
* 2026-03-20 | 0f892a6 | Clean dead helpers and fix validation regressions
* 2026-03-20 | 1dd2349 | Clean dead helpers and fix validation regressions
* 2026-03-20 | 389da50 | Clean dead helpers and fix validation regressions
* 2026-03-20 | fde49fe | Clean dead helpers and fix validation regressions
* 2026-03-20 | 24142a9 | Remove stale explorer allows and duplicate modules
* 2026-03-20 | 8835549 | Remove stale explorer allows and duplicate modules
* 2026-03-20 | 64c060b | Remove stale explorer allows and duplicate modules
* 2026-03-20 | 1dd1cdb | Remove stale explorer allows and duplicate modules
* 2026-03-21 | 9625df4 | Surface tool stream read failures
* 2026-03-21 | 06765b3 | Surface tool stream read failures
* 2026-03-21 | 2422d74 | Surface tool stream read failures
* 2026-03-21 | a34a1eb | Surface tool stream read failures
* 2026-03-21 | fcaddbe | Harden XMP matching and SSIM mapping
* 2026-03-21 | 9637e4c | Harden XMP matching and SSIM mapping
* 2026-03-21 | 5a3a75c | Harden XMP matching and SSIM mapping
* 2026-03-21 | 2552741 | Harden XMP matching and SSIM mapping
* 2026-03-21 | 24c1c62 | Harden XMP metadata discovery and sidecar matching
* 2026-03-21 | cb1b174 | Harden XMP metadata discovery and sidecar matching
* 2026-03-21 | cf975ee | Harden XMP metadata discovery and sidecar matching
* 2026-03-21 | 3a150ef | Harden XMP metadata discovery and sidecar matching
* 2026-03-21 | 130b13c | chore: sync changelog for v0.10.79/0.10.80 and update progress tracking logic
* 2026-03-21 | ca9e6a1 | chore: sync changelog for v0.10.79/0.10.80 and update progress tracking logic
* 2026-03-21 | 99b915c | merge nightly v0.10.80 into main (maintaining stable dependencies)
* 2026-03-21 | 745db2f | merge nightly v0.10.80 into main (maintaining stable dependencies)
* 2026-03-21 | 3e1a76e | feat: standardize output extensions to uppercase and fix formatting in simple mode
* 2026-03-21 | d875704 | feat: standardize output extensions to uppercase and fix formatting in simple mode
* 2026-03-21 | b8c5f32 | merge uppercase extensions and formatting fixes into main (maintaining stable dependencies)
* 2026-03-21 | e8efdb0 | merge uppercase extensions and formatting fixes into main (maintaining stable dependencies)
* 2026-03-21 | d397fb2 | merge nightly v0.10.81 into main (maintaining stable dependencies)
* 2026-03-21 | 32f73dd | merge nightly v0.10.81 into main (maintaining stable dependencies)
* 2026-03-21 | 5331988 | test: remove #[ignore] from all tests and fix stale assertions in video_explorer
* 2026-03-21 | 561a73d | test: remove #[ignore] from all tests and fix stale assertions in video_explorer
* 2026-03-21 | 95f608d | merge test fixes into main
* 2026-03-21 | c85dd89 | merge test fixes into main
* 2026-03-21 | deb5337 | feat: inject MFB branding into macOS Finder comments
* 2026-03-21 | fd57df7 | feat: inject MFB branding into macOS Finder comments
* 2026-03-21 | a57837a | merge macOS Finder branding into main
* 2026-03-21 | bcfa526 | merge macOS Finder branding into main
* 2026-03-21 | f20dabc | feat: restrict Finder branding to target formats (JXL, MOV, MP4)
* 2026-03-21 | cbc4148 | feat: restrict Finder branding to target formats (JXL, MOV, MP4)
* 2026-03-21 | 55ca01d | merge selective Finder branding
* 2026-03-21 | 5c5cb57 | merge selective Finder branding
* 2026-03-21 | b6ec5de | security: remove sensitive prompts from history and add to gitignore
* 2026-03-21 | 6f93369 | security: remove sensitive prompts from history and add to gitignore
* 2026-03-21 | b0c3d3c | chore: bump workspace version to 0.10.82
* 2026-03-21 | d7c56a7 | chore: bump workspace version to 0.10.82
* 2026-03-21 | 2af000d | merge version bump to 0.10.82
* 2026-03-21 | e90fdfc | merge version bump to 0.10.82
* 2026-03-21 | 374a797 | fix: atomic rename for Windows and FFmpeg stream mapping for cover art
* 2026-03-21 | 6dc59e3 | fix: atomic rename for Windows and FFmpeg stream mapping for cover art
* 2026-03-21 | 46d6c6b | merge v0.10.82 performance and stability fixes
* 2026-03-21 | abdc61e | merge v0.10.82 performance and stability fixes
* 2026-03-21 | dfa68f7 | Harden error visibility and recovery paths
* 2026-03-21 | ee42745 | Harden error visibility and recovery paths
* 2026-03-21 | 8e1133c | Tighten cleanup failure reporting
* 2026-03-21 | ddb3433 | Tighten cleanup failure reporting
* 2026-03-21 | e07c84f | Surface cache and ffprobe failures
* 2026-03-21 | aa0fabf | Surface cache and ffprobe failures
* 2026-03-21 | 7834948 | merge v0.10.82: comprehensive hardening, path security, and error visibility fixes
* 2026-03-21 | 5cb771c | merge v0.10.82: comprehensive hardening, path security, and error visibility fixes
* 2026-03-21 | 4a34187 | Pause batch runs on mid-process disk exhaustion
* 2026-03-21 | 452347f | Pause batch runs on mid-process disk exhaustion
* 2026-03-21 | 738f2b3 | merge v0.10.82 update: pause batch runs on disk exhaustion
* 2026-03-21 | 99cfc6f | merge v0.10.82 update: pause batch runs on disk exhaustion
* 2026-03-21 | a02000d | Scope Finder comment branding to conversion output only; surface delete failures
* 2026-03-21 | 6522058 | Scope Finder comment branding to conversion output only; surface delete failures
* 2026-03-21 | c2a0323 | Surface more silent runtime degradation paths
* 2026-03-21 | b4129a4 | Surface more silent runtime degradation paths
* 2026-03-21 | de6bbda | fix: scope Finder branding to conversion and surface more silent failures
* 2026-03-21 | 27a4f28 | fix: scope Finder branding to conversion and surface more silent failures
* 2026-03-21 | c119011 | merge v0.10.83: stability and metadata scoping fixes
* 2026-03-21 | ea17b65 | merge v0.10.83: stability and metadata scoping fixes
* 2026-03-21 | e6d063b | Improve perceived-speed scheduling and surface silent failures
* 2026-03-21 | d65d3bd | Improve perceived-speed scheduling and surface silent failures
* 2026-03-21 | 6d81807 | Improve perceived-speed scheduling and surface silent failures
* 2026-03-21 | c9f7ce6 | Improve perceived-speed scheduling and surface silent failures
* 2026-03-21 | e11f17b | Harden GUI launches and narrow-terminal progress
* 2026-03-21 | 20536ad | Harden GUI launches and narrow-terminal progress
* 2026-03-21 | d68b948 | Harden GUI launches and narrow-terminal progress
* 2026-03-21 | 8756c54 | Harden GUI launches and narrow-terminal progress
* 2026-03-21 | 59c8246 | merge v0.10.85: environment hardening and terminal-aware progress
* 2026-03-21 | 69b8f7c | merge v0.10.85: environment hardening and terminal-aware progress
* 2026-03-21 | 997b035 | chore: restore GitHub metadata and nightly patch section
* 2026-03-21 | 32c42d8 | chore: restore GitHub metadata and nightly patch section
* 2026-03-21 | d0b27ba | merge v0.10.85 (with GitHub sources)
* 2026-03-21 | 8de2b02 | Fix nightly GitHub dependency build regression
* 2026-03-21 | f3e51d4 | Fix nightly GitHub dependency build regression
* 2026-03-21 | d03f8a4 | Surface more silent failures and reset stale checkpoints
* 2026-03-21 | 2e180c9 | Surface more silent failures and reset stale checkpoints
* 2026-03-21 | b6331db | Tighten resume validation with cache-bound checkpoints
* 2026-03-21 | 0cb4a93 | Tighten resume validation with cache-bound checkpoints
* 2026-03-21 | 8ffb098 | Make checkpoint process probing portable and louder
* 2026-03-21 | 9d0dae5 | Make checkpoint process probing portable and louder
* 2026-03-21 | 937a49d | Finish surfacing startup and runtime state failures
* 2026-03-21 | 8a4217d | Finish surfacing startup and runtime state failures
* 2026-03-21 | dfed411 | Refine video CRF warm-start cache hints
* 2026-03-21 | 2bf8d57 | Refine video CRF warm-start cache hints
* 2026-03-21 | 11c4917 | Refine video CRF warm-start cache hints
* 2026-03-21 | fde43f7 | Make temp output suffix rand-api agnostic
* 2026-03-21 | 8b7d0e5 | merge v0.10.85: documentation and latest fixes
* 2026-03-21 | f790c24 | release: v0.10.86 - finalized v0.10.85 features and documentation
* 2026-03-21 | df4b355 | release: v0.10.86 - finalized v0.10.85 features and documentation
* 2026-03-21 | 425b2c2 | merge v0.10.86: sealed release with updated notes
* 2026-03-22 | 1004123 | docs: consolidate redundant documentation and release notes into docs/ directory
* 2026-03-22 | f5e2c94 | force sync nightly to remote to resolve diversion
* 2026-03-22 | 60aac8c | merge v0.10.86: synchronized after dual-branch privacy purge
* 2026-03-22 | 6bce313 | release: v0.10.87 - privacy hardened repository with segmented dependency architecture
* 2026-03-22 | ac0e2c3 | docs: re-anchor project documentation with complete README history purged
* 2026-03-22 | c2c372f | build(nightly): synchronize and update GitHub dependencies to latest upstream iterations (v0.10.87-nightly)
* 2026-03-22 | 272163e | docs: reconstruct and synchronize 2200-line changelog following repository sanitization (v0.10.87)
* 2026-03-22 | 5483742 | docs: finalize v0.10.87 changelog with comprehensive official release notes (v0.10.78-v0.10.87)
* 2026-03-22 | dddcb6b | docs: integrate core historical release notes (v0.10.66, v0.10.64, v0.10.9) into unified changelog
* 2026-03-22 | 5de8774 | docs/app: restore macOS application bundle stripped during repository sanitization
* 2026-03-22 | fc98820 | build: finalize and lock drag-and-drop scripts for v0.10.87 release
* 2026-03-22 | 4f58b56 | docs: integrate translated historical 'loud failure' notes into unified changelog (v0.10.82-v0.10.87)
* 2026-03-22 | 808bd25 | Fix odd-dimension metric normalization for animated quality checks
* 2026-03-22 | 571a92c | build: restore modern English-only macOS app bundle (v0.10.87)
* 2026-03-22 | d1d3f4c | build: finalize app bundle versioning to v0.10.87 (2026-03-22)
* 2026-03-22 | 281a65a | build: truly restore original v0.10.87 app bundle and changelog
* 2026-03-22 | 272ffb7 | build: remove redundant cleanup script and finalize unified project state
* 2026-03-22 | 4c078c9 | docs: RESTORED FULL ULTIMATE CHANGELOG via local Cursor history (2200+ lines)
* 2026-03-22 | 0312a7e | feat: add real-time branch/version transparency to UI header (v0.10.87)
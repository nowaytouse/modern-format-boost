## [0.10.102] - 2026-03-26

### 🛠️ Hardening & Technical Debt Cleanup
- **Quality & Performance**:
  - **Zero-Warning Workspace**: Achieved a clean, warning-free build across all crates (`img_hevc`, `img_av1`, `shared_utils`) and shell scripts.
  - **Dependency Update**: Full workspace-wide dependency synchronization via `cargo update` to the latest stable and nightly-compatible versions.
- **Image Intelligence**:
  - **EXR Detection**: Advanced attribute parsing for `OpenEXR` compression types (NONE/RLE/ZIPS/ZIP/PIZ for lossless; DWAA/DWAB etc. for lossy).
  - **JP2 Improvements**: Robust wavelet transform analysis (9/7 irreversible vs 5/3 reversible) via COD/COC marker scanning for precise lossy/lossless detection.
- **Core Refactoring**:
  - **img_hevc**: Major structural refactoring to align with `img_av1` architecture. Modularized the monolithic conversion logic into specialized dispatch functions, significantly reducing complexity while preserving the advanced video/static logic.
  - **img_av1**: Hardened the conversion pipeline with improved error mapping and consistent result reporting.
- **Shell Script Fortification**: Systematic resolution of all `shellcheck` warnings (SC2155, SC2086, etc.) across the script suite for enhanced reliability.
- **Bug Fixes**:
  - **GPU Coarse Search**: Fixed Sprint acceleration logic that was incorrectly resetting after first trigger, now allows continuous step doubling throughout the search phase for improved efficiency.
  - **Shell Path Detection**: Fixed `common.sh` to use `${(%):-%x}` for zsh when sourced, preventing incorrect path resolution in multi-script workflows.


## [0.10.101] - 2026-03-26

### 🛠️ Code Quality & Technical Debt
- **Zero-Debt Architecture**: Achieved 100% Clippy compliance across `shared_utils` by resolving all `pedantic` and `nursery` blockers.
- **Redundancy Elimination**: Consolidated redundant match arms in `quality_matcher.rs` and `unified_error.rs` (`match_same_arms`), reducing binary size and logic complexity.
- **Modern Rust Idioms**: Migrated nested error handling and option unwrapping to `let-else` syntax across 12 files (including `ssim_calculator.rs` and `gpu_accel.rs`), improving code flatness and readability.
- **Structural Standardization**: Corrected item declaration order in `modern_ui.rs` and `progress.rs` to satisfy `items_after_statements` lints.
- **Clean Documentation**: Fixed missing link targets and formatting issues in crate-root documentation.

### ✨ Features & Format Support
- **OpenEXR & JPEG 2000 Integration**: Restored missing detection logic for `.exr`, `.jp2`, and `.j2k` formats.
  - Finished the `detect_compression` dispatcher: these formats are now correctly identified as lossless/lossy at the binary level.
  - Native Pipeline Hook: `img_hevc` and `img_av1` now support these formats as direct inputs to `cjxl` without unnecessary intermediate conversions.
- **Script Enhancements**: Enhanced `scripts/check_all.sh`:
  - Added `--fix` flag for automatic code formatting and clippy fixes.
  - Removed unused variables, added null checks, fixed syntax errors.
  - Translated remaining Chinese comments to English.

## [0.10.100] - 2026-03-25

### 🧪 Compatibility & Maintenance
- **Legacy ICC Rounding Logic**: Added an on-demand retry path for an edge-case where very old `cjxl` versions (<= v0.10) might reject ICC profiles due to D50 rounding issues. **Note:** Verified as non-triggering/inactive on modern `cjxl v0.11.2` due to improved upstream tolerance. The logic remains purely as a non-intrusive safety net for legacy toolchains.
- **JXL Container Handling**: Confirmed that `exiftool` (with `-m`) automatically handles containerization requirements for JXL codestreams. Reverted the unnecessary `--container=1` flag to maintain output purity.
- **Dependency Refresh**: Updated 8 core crates to their latest security/bugfix releases.

### 🔍 Diagnostics
- **Silent Fallback Logging**: All previously invisible fallback events now emit to log files (`DEBUG`/`WARN` level). Specifically: `exiftool` stderr (including `-m`-suppressed warnings) is now captured via `tracing::debug!`; `cjxl` decode failures that trigger the ImageMagick or FFmpeg fallback pipelines now emit `tracing::warn!` with the full upstream error before the retry begins. Terminal output is unchanged — all new entries are file-only.


## [0.10.99] - 2026-03-24

### ✨ Features
- **Robust Quality Metrics for Animated Sources**: Implemented "Compatible Quality Measurement Mode" for GIF, WebP, AVIF, HEIC, and APNG. The system now automatically switches to a more robust `SSIM-All` calculation (with format normalization and alpha flattening) if the fast SSIM path fails, ensuring consistent metrics across iterations.
- **Probe-First Format Identification**: Upgraded animated image detection to prioritize `ffprobe format_name` over simple file extensions. This ensures files with non-standard extensions (e.g., `2.gif.file`) are correctly routed to the relaxed animation processing pipeline instead of strict video paths.

### 🐛 Bug Fixes
- **GPU SSIM Resilience**: Refined GPU SSIM baseline handling to prevent interruptions when metrics measurements are unavailable, allowing the search to proceed seamlessly using CPU-based diagnostics.

## [0.10.98] - 2026-03-24

### 🐛 Bug Fixes
- **GPU SSIM Baseline Tolerance**: Refactored `gpu_coarse_search.rs` to treat missing GPU SSIM baseline as a non-fatal warning. The search now gracefully continues with CPU delta-only exploration instead of bailing, improving reliability on systems with transient GPU metric failures.
- **Temp File Lifecycle Management**: Implemented `TempOutputGuard` across all animated image conversion paths in `vid_hevc` and `vid_av1`. Ensures automatic cleanup of `*.tmp.*` files even during early returns or error propagation (`?`), preventing disk clutter from abandoned temporary artifacts.

### 🛠️ Code Quality
- **Branch Synchronization**: Synchronized `main` and `nightly` branches with unified fix implementations while maintaining separate dependency philosophies (crates.io for main, GitHub/Git for nightly).

## [0.10.97] - 2026-03-24

### 🛠️ Code Quality
- **Integrity Protection Removal**: Decoupled the build process from documentation content by removing the README/CHANGELOG signature verification mechanism.

## [0.10.96] - 2026-03-24

### 📝 Documentation & Localization
- **Total Linguistic Standardization**: Translated all remaining Simplified Chinese comments and documentation headers to professional technical English across the entire `shared_utils` crate.
- **Improved Code Readability**: Standardized documentation style for core modules including `terminal_logging`, `ffprobe_json`, `explore_strategy`, and the `types` submodule.
- **Unicode Test Path Optimization**: Updated test paths in `path_validator.rs` to English while maintaining coverage for non-ASCII path handling.

### 🛠️ Code Quality
- **Clippy Hardening**: Addressed remaining clippy warnings to ensure a 100% clean build in `shared_utils`.
- **Macro Documentation**: Corrected and translated doc-comments for logging macros.

## [0.10.95] - 2026-03-24

### 🛠️ Code Quality (Shared Utils)
- **Pedantic Clippy Hardening**: Achieved zero warnings in `shared_utils` (standard/pedantic) by addressing:
    - `redundant_else`: Removed unnecessary `else` blocks after `return`/`break` in `gpu_accel.rs`, `quality_matcher.rs`, and `video_detection.rs`.
    - `similar_names`: Applied `#[allow]` attributes to contextually appropriate naming (e.g., `ctime`/`btime` in cache, `vmaf`/`uvmaf` in video metrics).
    - `missing_errors_doc` & `missing_panics_doc`: Added required documentation sections to public APIs in `checkpoint.rs`, `conversion.rs`, and `terminal_logging.rs`.
    - `uninlined_format_args`: Inlined variables in `format!` macros across the crate.
    - `unused_self`: Refactored `enhanced_logging.rs` to correctly acknowledge `self`.
    - `map_unwrap_or`: Replaced with more idiomatic `map_or` in `checkpoint.rs`.
- **Syntax Integrity**: Fixed a regression in `gpu_accel.rs` caused by redundant delimiter removal during clippy fixing.
## [0.10.94] - 2026-03-23

### 🛠️ Code Quality Tooling
- **`scripts/check_all.sh` Reliability Rewrite**: Reworked the quality scan script with strict shell safety (`set -Eeuo pipefail`), deterministic repo-root execution, and structured pass/fail/warn/skip summaries.
- **Nightly-First Branch Policy**: Added default git-branch enforcement to run checks on `nightly` unless explicitly bypassed with `--allow-non-nightly`.
- **Required vs Optional Gates**: Split checks into required gates (`fmt`/`clippy`/tests) and optional deep scans, with required failures now correctly returning a non-zero exit code.
- **Installed Tool Awareness**: Optional checks now auto-detect installed Cargo subcommands (`audit`, `deny`, `machete`, `udeps`, `geiger`, `bloat`, `hack`, `miri`) and skip missing tools with explicit reasons.
- **Network-Safe Security Checks**: `cargo audit` and `cargo deny` default to no-fetch mode for stable local runs, with an opt-in `--fetch-advisory-db` switch when fresh advisory sync is needed.
- **Operational Modes**: Added `--required-only`, `--no-expensive`, and help output for CI and local debugging workflows.
- **Sandbox-Aware Deny Handling**: `check_all.sh` now auto-skips `cargo deny` when the advisory DB path is read-only or missing, preventing false-negative warnings in restricted environments.

### 🐛 Quality Fixes
- **Clippy Compliance (Shared Utils)**: Fixed strict lint blockers in `ffmpeg_process.rs` by replacing newline `write!` with `writeln!` and simplifying `JoinHandle` result handling with `unwrap_or_else`.
- **HEVC Strategy Test Compilation Repair**: Updated `vid_hevc/src/conversion_api.rs` tests to match the current `determine_strategy_with_apple_compat(result, apple_compat, force)` signature.
- **Filesystem-Safe Test Paths**: Reworked affected image converter tests (`img_av1` and `img_hevc`) to use `tempfile` + canonicalized temp roots instead of hard-coded absolute paths (e.g. `/path`, `/output`, `/var`) that violate current path safety rules.
- **Integrity Signature Refresh**: Updated `shared_utils/src/version.rs` expected README/CHANGELOG signatures to match current normalized documentation content after changelog updates.
- **Formatting Consistency**: Applied `cargo fmt --all` to keep workspace formatting and CI checks aligned.
- **Unused Dependency Cleanup**: Removed stale direct dependencies in `shared_utils`, `vid_av1`, `vid_hevc`, `img_av1`, and `img_hevc` so `cargo machete` reports zero unused crates.
- **Workspace Patch Hygiene**: Removed unused `rand`/`rand_core` `[patch.crates-io]` overrides from root `Cargo.toml` to eliminate cargo patch-noise warnings.

## [0.10.93] - 2026-03-23

### 🐛 Bug Fixes
- **cjxl Process Termination Detection**: Enhanced error handling in `jxl_utils.rs` to properly detect when cjxl is terminated by signal (SIGKILL/SIGSEGV). Now logs "Process terminated by signal (possible crash or OOM kill)" instead of generic "exit code: None", helping diagnose OOM issues.
- **FFprobe Warning Reduction**: Improved error filtering in `ffprobe_json.rs` to reduce unnecessary warnings for JPEG/image files where ffprobe failure is expected (not video streams). Only logs warnings for genuine errors.
- **ImageMagick Fallback Error Messages**: Added detailed error context when all ImageMagick+cjxl pipeline attempts fail, explaining possible causes (corrupted data, unsupported format, cjxl crash/OOM).

### ⚡ Performance & Memory Management
- **Enhanced OOM Prevention**: Strengthened memory pressure detection thresholds in `system_memory.rs`:
  - Low pressure: now requires 30% available RAM (up from 25%) and 3GB minimum (up from 2GB)
  - Normal pressure: now requires 15% available RAM (up from 10%) and 1.5GB minimum (up from 1GB)
- **Aggressive Parallelism Caps**: Updated `thread_manager.rs` to cap parallel tasks at 6 and child threads at 4 even under low memory pressure, preventing sudden memory spikes during heavy image processing operations (cjxl/ImageMagick).
- **Multi-Instance Optimization**: Improved thread allocation to better handle concurrent instances, reducing OOM risk when multiple conversion processes run simultaneously.


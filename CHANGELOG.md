# Changelog

All notable changes to this project will be documented in this file.

**Version scheme:** As of this release, the project uses **0.8.x** versioning (replacing the previous 8.x scheme).

## [0.11.2] — 2026-04-05

#### 🌈 Ultra HDR Migration Pipeline (Gain Map Support)

- **Migration Path B (SDR + Sidecar)**: Seamlessly detects Google Ultra HDR JPEGs and reroutes them via `generate_jxl_indicator` into a dedicated migration workflow.
- **Bit-Perfect Base Image**: The SDR base of the UltraHDR image is recompressed identically into `JXL` utilizing `cjxl --lossless_jpeg=1`, achieving ~10% size shrinkage without losing decoding fidelity.
- **Sidecar Extraction**: Automatically extracts the Google Gain Map segment via Multi-Picture Format (MPF) detection and preserves it as an adjacent `.gainmap.png` sidecar file for downstream HDR reconstruction.
- **XMP Metadata Preservation**: Uses `ExiftoolBuilder` to robustly bridge raw `hdrgm` tags into the new JXL container.
- **Technical Debt Resolved**: Replaced legacy `meme_score` nomenclatures (`directory_meme_score`, `filename_meme_score`) with mathematically standardized `loop_intent_score` variables across Rust and Python ecosystems.

#### 📦 Dependency Modernization (Routine)

- **Library Refresh**: Updated core processing crates (`image`, `chrono`, `tracing`, etc.) to the latest stable versions via automated workspace sync (`cargo update`).

#### 🔧 KNN Class Imbalance Stabilization & Logging Cleanup

- **Stabilized KNN under class imbalance**: Replaced hard inverse-frequency scaling with **smoothed+damped class-balance weights**, added **Beta-smoothed global prior** and **effective-sample-size shrinkage** (`local posterior ↔ global prior`) so minority classes are protected without causing prediction cliffs under extreme dataset imbalance.
- **Confidence anti-slope guard**: KNN confidence now includes imbalance and effective-neighbor penalties to avoid overconfident flips when nearest neighbors are sparse or class distribution is highly skewed.
- **Debug observability for balancing math**: Added structured `DEBUG` logs for KNN balancing internals (`w_keep/w_weak`, global prior, imbalance ratio, effective-N, shrink factor, posterior) to support on-data tuning without polluting terminal output.
- **Moved KNN internals to `DEBUG` level**: KNN confidence/neighbor count logs, fallback result messages, and database bootstrap lines now emit to `DEBUG` instead of regular terminal channels, providing a much cleaner terminal experience.
- **Fixed temporal BPP formula bug**: Legacy code in `lookup_similar_samples_inner` multiplied by `frame_count` instead of dividing — corrected to use proper per-frame density calculation (`density / frames`).
- **Extracted `bpp_from_meta` helper**: Consolidated duplicate temporal/spatial BPP calculation logic in `database.rs` into a single reusable function with clearer semantics.
- **Added regression test**: `bpp_from_meta_divides_temporal_density_by_frame_count` validates the corrected formula against legacy buggy behavior.

#### 🛡️ Path Safety & Media Integrity Hardening

- **Relativization Shield**: Mitigated ImageMagick 7 absolute path truncation bugs by implementing mandatory `./` guarding for all file inputs.
- **ExifTool Injection Defense**: Hardened `exiftool_path_arg` with unconditional `./` guarding to prevent command hijacking via `-` or `@` filename prefixes.
- **Format Expansion Prevention**: Implemented double-percent (`%%`) escaping to lock down filename property expansion vulnerabilities.
- **Shell Injection Defense**: Added metacharacter scanning and protocol-less relative addressing to prevent command injection via ImageMagick delegates.
- **URI-compliant Pathing**: Implemented the `file:///` (triple-slash) protocol in `magick_safe_path` for 100% stable absolute path preservation.
- **Metadata Bomb Stamina**: Hardened the XMP/EXIF pipeline against abnormally high metadata density, preventing OOM and hangs during concurrent processing.
- **Zero-Duration Rhythm Lockdown**: Implemented strict validation to reject media with invalid inter-frame delays, preventing high-speed playback artifacts.

#### 🧹 Code Quality & Clippy Hygiene (Updated 2026-04-06)

- **Workspace-wide Clippy Compliance**: Resolved numerous `pedantic` and `nursery` warnings across `shared_utils`, `vid`, and `img` crates.
- **Numeric safety**: Eliminated unsafe `as` numeric casts across the workspace by migrating to centralized `numeric_cast` module with saturating helpers.
- **Clippy pedantic cleanup**: Resolved warnings for `similar_names`, `large_stack_arrays`, `while_immutable_condition`, `collapsible_if`, and `assigning_clones` across `shared_utils`, `vid`, and `dev` crates.
- **Idiomaticity**: Improved code by replacing manual `match` or `if let` blocks with `let-else`, `map_or_else`, and `and_then` where appropriate.
- **Performance Optimization**: Removed redundant `clone()` calls and utilized `unwrap_or_else` to avoid unnecessary allocations in hot paths.
- **Structural Integrity**: Renamed unused required struct fields in `database.rs` with underscore prefixes to satisfy `dead_code` analysis while maintaining DB compatibility.
- **Concurrency**: Tightened Mutex lock scopes in `checkpoint.rs` and `conversion.rs` to minimize potential resource contention.
- **Formatting consistency**: Reformatted long argument chains, `format!()` → inline `{var}` syntax, and multi-line function calls across 50+ files for improved readability.
- **Blake3 buffer heap allocation**: Converted a hot 64KB stack allocation buffer into a Heap allocation to prevent stack overflows on heavily loaded multi-threaded architectures.
- **Dead code removal**: Removed unused `relative_distance` helper and simplified quality-ceiling `Option` handling in `gpu_accel.rs`.
- **Stage 3 spin safety cap**: Replaced `while_immutable_condition` allow with an explicit spin counter safety cap in GPU coarse search.
- **Documentation**: Fixed missing backticks in HDR synthesis documentation and standardized long numeric literals with underscores (e.g., `500_000.0`) for improved readability.

#### 🎬 Video Explorer & GPU Coarse Search Improvements

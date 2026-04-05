# Changelog

All notable changes to this project will be documented in this file.

**Version scheme:** As of this release, the project uses **0.8.x** versioning (replacing the previous 8.x scheme).

## [0.11.2] — 2026-04-05

#### 🎬 Media Integrity & GIF Playback Rhythm (Rhythm Fixes)

- **Strict Data-Driven FPS**: Implemented a 100% physical-fact calculation for GIF conversion: `FPS = (实际提取帧数) / (原始时长)`. This fixes the "Ghost Rhythm" (hyper-speed鬼畜) issue in AVIF-to-GIF conversions.
- **Zero Numerical Tampering (Anti-Tampering)**: Completely removed all "silent magic-number fallbacks" (e.g., 20.0 or 25.0 FPS) for missing metadata. If timing information cannot be derived from source data, the conversion now fails with a clear error instead of guessing.
- **Enhanced Bit-Depth Accuracy**: Refactored `ffprobe.rs` to derive bit depth directly from `pix_fmt` strings (e.g., `yuv420p10le`) rather than defaulting to 8-bit, ensuring faithful color rendering.
- **Average Frame Rate Support**: Added `avg_frame_rate` to the core `FFprobeResult` schema, allowing for more accurate playback speed detection in Variable Frame Rate (VFR) containers.
- **Alpha Protection (Transparency Reinforcement)**: Enforced explicit `RGBA` pixel format across the entire extraction and alpha-merging pipeline. This prevents transparency-to-black bleeding and ensures professional color accuracy for transparent animated images (WebP/AVIF/GIF).
- **Professional Log Standardization**: Audited and simplified the `cli_runner.rs` terminal output, removing decorative Emojis from core processing paths to ensure professional log clarity.
- **Type-safe Metadata Builder**: Refactored `ExiftoolBuilder` to provide high-level methods like `.quiet()`, `.ignore_minor()`, and `.tags_from_file()`, eliminating redundant raw command-line strings across the codebase.
- **Log Silence (Zero-Noise)**: Suppressed non-actionable `ExifTool` warnings (e.g., "No writable tags set from JXL") via dual-quiet flags and proper `stderr` piping in the concurrent `XmpMerger` pipeline, ensuring a clean and focused terminal output.

#### 🏗️ Architecture: Strict Static vs. Animated Module Isolation (img & vid)

- **Fixed static modern format detection**: Updated `SourceCodec::is_animated()` to remove default animation flags for AVIF and HEIC. These formats are now treated as static by default until container analysis confirms an image sequence.
- **Added "Single-Frame Interception" in vid**: Implemented a mandatory check in the `vid` conversion pipeline (`auto_convert_with_cache`). If `frame_count <= 1`, the file is identified as a static image and skipped by `vid`, ensuring it is handled by the `img` module for optimal JXL encoding.
- **Cleaned up animation capability metadata**: Removed `WebpStatic` from `can_be_animated()` to prevent misrouting static WebP files to the animated media pipeline.
- **Expanded video extensions**: Added `gif`, `webp`, `avif`, `heic` to `supported_video_extensions` to ensure the `vid` tool correctly scans potential animated candidates.
- **Content-Based Media Identification**: Implemented `SourceCodec::identify_by_content` using magic-byte detection (16-byte header probe), ensuring accurate format identification even with incorrect file extensions.
- **Auto-Correction of Extensions**: Refactored `smart_file_copier.rs` and `cli_runner.rs` to automatically correct file extensions based on content before processing.
- **New Classification Metrics (Loop Intent)**: Integrated advanced temporal analysis into the classification logic:
  - **Motion Periodicity**: Measures rhythmic regularity of motion vectors to identify looping sequences.
  - **Temporal Jitter**: Analyzes PTS (Presentation Time Stamp) regularity to detect consistent frame timing.
  - **Loop Closure Score**: Enhanced detection of seamless transitions between the end and start of a sequence.

#### 🗄️ Persistent Cache & Forensic Schema (v3)

- **Database Schema Upgrade (v2 -> v3)**: Incremented `CACHE_SCHEMA_VERSION` to `3` to implement content-addressable caching.
  - **BLAKE3 Content Fingerprinting**: Added `content_fingerprint_hash` column to both image and video analysis tables. The system now uses BLAKE3 hashing to verify file identity, making the cache immune to path/mtime collisions.
  - **Data Integrity**: Added `data_checksum` column for storing verification hashes of the processed results.
  - **Automated Migration**: Implemented a robust `check_and_migrate_schema()` workflow that detects v2 databases and performs non-destructive `ALTER TABLE` operations to inject new forensic columns.
- **Improved PostgreSQL Support**: Synchronized the `analysis_cache_pg.sql` schema with the SQLite implementation, ensuring feature parity across local and remote cache backends.
- **Database Safety & Robustness**:
  - Implemented `is_finite` safety checks for all floating-point logging in both SQLite and PostgreSQL backends to prevent training data corruption.
  - Enhanced error reporting in `image_quality_db.rs` to include the final verdict in non-fatal logging failures.

#### 🖥️ App Wrapper & Platform Safety
- **Major App Script Refactoring**: Completely rewrote the macOS App entry point (`Modern Format Boost` binary).
  - Implemented robust `PYTHON_BIN` discovery (checks `.venv`, system python3, and `/usr/bin/python3`).
  - Enhanced path security with `escape_shell_double_quotes` and improved AppleScript string escaping.
  - Switched to `exec /bin/zsh -f -c` for terminal execution to ensure a clean, predictable shell environment.
- **Improved State Management**: Added `MFB_HOME_ROOT` logic to `drag_and_drop_processor.py`. When launched from the App, it now defaults to an isolated `.cache/mfb_runtime` directory instead of the user's home folder.
- **UI & Flow Control**: Added `ReturnToHomeException` and a main retry loop to the processor script, allowing the system to return to the selection menu after specific errors (like insufficient disk space) instead of exiting.
- **Progress UI Synchronization**: Added `reset_session_stats()` in `progress_mode.rs` to ensure terminal progress counters (e.g., `V:12✓`) accurately reflect the current directory processing task instead of cumulative session totals.

#### 🔬 Quality Training & Database Enhancements

- **Targeted Sample Ingestion**: Added `--label` support to the `vid ingest-samples` command, allowing for categorized training data collection.
- **Extension Filtering**: Hardened `train_quality.rs` with explicit image extension filtering (JPG, PNG, WebP, AVIF, HEIC, JXL) to prevent non-image files from polluting the quality database.
- **Training Pipeline**: Updated `training_pipeline.py` to include the `video` label map, aligning the ML model with the new multi-modal classification strategy.
- **BPP Formula Calibration**: Refined the Bit-Per-Pixel (BPP) heuristic formula in `image_detection.rs` for more accurate quality estimation across diverse image formats.

#### 🐞 Bugfixes & Stability

- **Animated AVIF to GIF Reliability**: Fixed a critical bug where `gifski` would fail on multi-stream animated AVIFs. Implemented a robust frame extraction pipeline (`ffmpeg` -> PNG sequence -> `gifski`) that ensures all frames are correctly captured and timed according to source duration.
- **AVIF Alpha Stream Detection**: Added heuristic logic to detect and accurately map auxiliary alpha streams (`yuv420p` + `gray8`) in animated AVIFs, preventing transparency loss during conversion.
- **Apple Compatibility Enforcement**: Fixed a bug where `apple_compat` mode incorrectly allowed copying incompatible original files (AVIF/WebP) to the output. The system now strictly enforces conversion to GIF/HEIC for Apple ecosystem compatibility.
- **Enhanced GIF Pipeline Safety**: Replaced direct single-file input for `gifski` with a managed pattern-based input system, eliminating "Only a single image file was given" errors.
- **Single-Frame Loop Veto**: Added "Layer 1-A" logic in `loop_intent.rs` to strictly reject single-frame media from being classified as loop assets, preventing misrouting of static images to the `gifski` pipeline.
- **Skip Reporting Transparency**: Enhanced `cli_runner.rs` with verbose logging for skipped files (checkpoint hit or output existing), resolving user confusion regarding progress bar increments without new output.
- **Checkpoint Resilience**: Audited `CheckpointManager` initialization to ensure progress directory persistence across process restarts.

#### 🧹 Maintenance & Documentation

- **Workspace Cleanup**: Deleted legacy documentation files (`docs/BRANCH_STRATEGY.md`, `docs/VERSION_MANAGEMENT.md`, `docs/decision_tree.md`) as the versioning and routing logic is now self-documenting in code.
- **Semantic Refactoring**: Completed a major semantic refactoring of the media classification system and reorganized the project structure for long-term maintainability.
- **Workspace Reorganization**: Migrated all core crates (`shared_utils`, `vid`, `img`) into a unified `crates/` directory.
- **Explicit State Management**: Eliminated ambiguous tri-state `Option<bool>` logic. Definitive metadata (HDR flags, audio presence, B-frames, etc.) is now handled as explicit `bool` or descriptive enums.
- **Granular Quality Reporting**: Refactored `CheckResult` to carry specific failure reasons for improved debuggability.

## [0.11.1] — 2026-04-04

#### 🏗️ Workspace Unification — Unified Media Architecture

Consolidated the previous HEVC-only and AV1-only crates into a single, unified codebase that supports both encoding strategies via dynamic dispatch.

# Changelog

All notable changes to this project will be documented in this file.

**Version scheme:** As of this release, the project uses **0.8.x** versioning (replacing the previous 8.x scheme).

## [0.11.2] — 2026-04-05

#### 🏗️ Architecture: Strict Static vs. Animated Module Isolation (img & vid)

- **Fixed static modern format detection**: Updated `SourceCodec::is_animated()` to remove default animation flags for AVIF and HEIC. These formats are now treated as static by default until container analysis confirms an image sequence.
- **Added "Single-Frame Interception" in vid**: Implemented a mandatory check in the `vid` conversion pipeline (`auto_convert_with_cache`). If `frame_count <= 1`, the file is identified as a static image and skipped by `vid`, ensuring it is handled by the `img` module for optimal JXL encoding.
- **Cleaned up animation capability metadata**: Removed `WebpStatic` from `can_be_animated()` to prevent misrouting static WebP files to the animated media pipeline.
- **Expanded video extensions**: Added `gif`, `webp`, `avif`, `heic` to `supported_video_extensions` to ensure the `vid` tool correctly scans potential animated candidates.
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

#### 🔬 Quality Training & Database Enhancements

- **Targeted Sample Ingestion**: Added `--label` support to the `vid ingest-samples` command, allowing for categorized training data collection.
- **Extension Filtering**: Hardened `train_quality.rs` with explicit image extension filtering (JPG, PNG, WebP, AVIF, HEIC, JXL) to prevent non-image files from polluting the quality database.
- **Training Pipeline**: Updated `training_pipeline.py` to include the `video` label map, aligning the ML model with the new multi-modal classification strategy.
- **BPP Formula Calibration**: Refined the Bit-Per-Pixel (BPP) heuristic formula in `image_detection.rs` for more accurate quality estimation across diverse image formats.

#### 🐞 Bugfixes & Stability

- **Animated AVIF to GIF Reliability**: Fixed a critical bug where `gifski` would fail on multi-stream animated AVIFs. Implemented a robust frame extraction pipeline (`ffmpeg` -> PNG sequence -> `gifski`) that ensures all frames are correctly captured.
- **AVIF Alpha Stream Detection**: Added heuristic logic to detect and accurately map auxiliary alpha streams (`yuv420p` + `gray8`) in animated AVIFs, preventing transparency loss during conversion.
- **Apple Compatibility Enforcement**: Fixed a bug where `apple_compat` mode incorrectly allowed copying incompatible original files (AVIF/WebP) to the output. The system now strictly enforces conversion to GIF/HEIC for Apple ecosystem compatibility.
- **Enhanced GIF Pipeline Safety**: Replaced direct single-file input for `gifski` with a managed pattern-based input system, eliminating "Only a single image file was given" errors.

#### 🧹 Maintenance & Documentation

- **Workspace Cleanup**: Deleted legacy documentation files (`docs/BRANCH_STRATEGY.md`, `docs/VERSION_MANAGEMENT.md`, `docs/decision_tree.md`) as the versioning and routing logic is now self-documenting in code.
- **Semantic Refactoring**: Completed a major semantic refactoring of the media classification system and reorganized the project structure for long-term maintainability.
- **Workspace Reorganization**: Migrated all core crates (`shared_utils`, `vid`, `img`) into a unified `crates/` directory.
- **Explicit State Management**: Eliminated ambiguous tri-state `Option<bool>` logic. Definitive metadata (HDR flags, audio presence, B-frames, etc.) is now handled as explicit `bool` or descriptive enums.
- **Granular Quality Reporting**: Refactored `CheckResult` to carry specific failure reasons for improved debuggability.

## [0.11.1] — 2026-04-04

#### 🏗️ Workspace Unification — Unified Media Architecture

Consolidated the previous HEVC-only and AV1-only crates into a single, unified codebase that supports both encoding strategies via dynamic dispatch.

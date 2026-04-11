# Release Notes — v0.11.2 (2026-04-11)

This version represents a significant hardening and refinement of the media encoding pipelines (HEVC, AV1, and JXL), transitioning the codebase to a convergent search model that prioritizes precision and visual fidelity.

## ✨ Highlights

### 🎞️ Video Pipeline Overhaul (HEVC / AV1)
- **Simplified HEVC Ultimate Logic**: Removed over 360 lines of complex candidate ranking logic in favor of a streamlined two-step process: efficiently converging at a search preset (`slow`) and finalizing with the high-fidelity delivery preset (`slower`).
- **Stage 5 Downward Exploration**: Implemented a post-search "boundary push" that prods the finalized CRF at 0.01 precision. It extracts every possible bit of quality until the file size matches the original input, ensuring users get the absolute best output for their budget.
- **Baseline-Aware Quality Gates**: Replaced static quality thresholds with adaptive, per-file baselines (VMAF-Y, PSNR-UV, and CAMBI). The system now dynamically detects and rejects pathological quality drops while remaining permissive for already degraded source material.
- **Improved Loop Stability**: Added iteration caps and tightened failure budgets (from 8 to 2) to prevent infinite oscillations on "uncompressible" source material.

### 🖼️ JXL (JPEG XL) Encoding Optimization
- **Rebuilt Exploration Algorithm**: Introduced an adaptive planning system that selects from four exploration profiles (MicroAdjust to CeilingSweep) based on the image's initial compression ratio.
- **Binary Search Convergence**: Replaced heuristic "jogging" with a high-precision binary search (0.0001 delta) for sub-0.01 distance values.
- **Stage 5 Sidekick**: Mirrored the video pipeline's "boundary push" to squeeze maximum quality from JXL finalization.

### 🛡️ Core Hardening
- **Logic Unification**: Standardized all size-comparison semantics to **strictly less than (<)** across all modules. If an output isn't smaller than the input, it is now discarded as a failed compression.
- **Dependency Stabilization**: Migrated the production `main` branch to use stable `crates.io` dependency sources, resolving several nightly-only API breakages in the process.
- **Source Directory Protection**: Hardened "source immutability" logic to ensure files are never modified or renamed in the input directory when a separate output directory is configured.

## 🔧 Maintenance
- Structural migration of core crates into a managed workspace (`crates/`).
- Resolved all floating-point comparison warnings and Python linting violations.
- Fixed a data-loss bug in the GIF skip-and-copy pipeline.

---
*For full technical details, see the detailed CHANGELOG.md.*

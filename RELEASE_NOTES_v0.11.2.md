# Release Notes — v0.11.2 (2026-04-11)

This release marks the culmination of the "Architecture Unification & Pipeline Hardening" cycle. It consolidates all improvements made since version **0.11.0** (2026-03-28), transitioning the project into a single, high-precision media workstation.

## 🚀 Key Highlights (Since 0.11.0)

### 🏗️ 1. Unified Media Architecture (v0.11.1 Milestone)
The project structure has been completely overhauled to eliminate redundancy and simplify deployment:
- **Binary Consolidation**: Merged `vid_hevc`/`vid_av1` into a single `vid` binary and `img_hevc`/`img_av1` into a single `img` binary. Users can now toggle between strategies using `--codec <hevc|av1>`.
- **Workspace Modernization**: Core Logic moved to a managed `crates/` workspace (`shared_utils`, `vid`, `img`), ensuring sub-100ms incremental build times and shared dependency management.

### 🎞️ 2. Video Pipeline Hardening & "Stage 5" Exploration
- **Stage 5 Downward Exploration**: After finding the optimal CRF, the system now performs a high-precision "boundary push" (0.01 delta) to extract every possible bit of quality until the file size matches the input budget.
- **HEVC Ultimate Simplification**: Replaced over 360 lines of candidate-ranking logic with a streamlined two-step encode: Search (`slow`) → Render (`slower`). This provides identical archival quality with significantly higher predictability.
- **Adaptive Quality Gates**: Replaced static VMAF/PSNR floors with per-file adaptive baselines. The system now dynamically detects and rejects quality regressions based on the source's native fidelity.

### 🖼️ 3. JXL (JPEG XL) Next-Gen Exploration
- **Adaptive Probing**: Introduced a profile-driven exploration model (MicroAdjust to CeilingSweep) that adapts its search strategy based on the image's "oversize pressure."
- **Binary Search Convergence**: Replaced heuristic "jogging" with an high-precision binary search (0.0001 delta) for sub-0.01 distance values.
- **Continuous Quality Sweep**: Implemented a JXL-specific "Stage 5 Sidekick" that Steps downward to explore higher visual fidelities until the first size regression.

### 🧠 4. Intelligence & Decision Flow (Loop Intent v3)
- **7-Layer Loop Intent System**: A multi-tiered heuristic+ML tree that accurately distinguishes between "sticker-like" loops and "narrative-like" clips.
- **pgvector HNSW Integration**: Migrated KNN similarity search from in-memory Euclidean math to PostgreSQL's HNSW vector index, enabling millisecond-level retrieval across thousands of labeled samples.
- **Level 4 Inference Logging**: The system now logs every internal signal and final decision to a structured forensic table, enabling data-driven tuning of the Active Learning loop.

### 🛡️ 5. UltraHDR & Media Integrity
- **Gainmap Resilience**: Implemented a four-strategy "candidate recovery" system for Google UltraHDR JPEGs, supporting malformed MPF segments and XMPF identifiers.
- **Baseline-Safe Synthesis**: Corrected transfer curve tagging (PQ/ST2084) for JXL HDR synthesis, ensuring proper "glow" and brightness in macOS Preview and web browsers.
- **Path Safety Shield**: Hardened all external tool calls (FFmpeg, ExifTool, cjxl) with standardized `./` prefixing and shell-escaping to defend against malicious filenames and protocol hijacking.

## 🔧 Core Maintenance
- **100% Health Milestone**: The entire codebase (Rust, Python, Shell) now passes the strict `check_all.py` quality suite with zero warnings/failures.
- **Logic Unification**: Standardized all efficiency metrics to **strictly less than (<)**.
- **Dependency Stabilization**: Reverted the production `main` branch to stable `crates.io` sources (fixing API breakages in `quick-xml` and `image`).
- **macOS App Wrapper**: Re-engineered the entry-point script with robust Python discovery and improved AppleScript escaping for drag-and-drop workflows.

---
*For a full itemized list of every commit and fix, please refer to the primary CHANGELOG.md.*

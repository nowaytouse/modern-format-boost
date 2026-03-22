### ✨ Highlights & New Features
- 🎞️ **HDR10+ Dynamic Metadata Support**: MFB now fully supports the extraction and retention of SMPTE 2094-40 dynamic metadata.
    - Utilizes `hdr10plus_tool` for precise SEI message extraction to JSON sidecars.
    - Re-injects metadata into `x265` via `--dhdr10-info` to ensure a consistent high-dynamic-range experience on supported displays.
    - Implemented a "Strict-first, Skip-validation-fallback" extraction strategy to handle non-standard or slightly non-compliant source files gracefully.
- ⚡ **Perceived Speed Optimization (Quick-Wins Sorting)**:
    - Introduced a multi-dimensional sorting engine in `shared_utils/src/batch.rs`.
    - Prioritizes "shallow" files and "lower-workload" media (smaller resolutions/durations) to provide instant visual feedback and maximize tool throughput from the first second.
- 🛡️ **Intelligent Checkpoint & Resume Reset**: 
    - Enhanced the `CheckpointManager` to be aware of the output destination's state.
    - Manually deleting the output directory (e.g., `_optimized`) now acts as a "hard reset" signal, automatically clearing stale progress records and triggering a full re-conversion, solving the "skipped processed files" bug.

### 🛠️ Core Engine Reliability
- 🧪 **MS-SSIM/VMAF Fusion Pipeline Hardening**:
    - **Exit Code Tolerance**: FFmpeg is now allowed to finish with non-zero exit codes if valid JSON metric data is present in stdout, eliminating false "Pixel format incompatibility" errors.
    - **Chroma Resolution Guard**: Added a safety threshold (256×256 min) for MS-SSIM chroma channels. If a video's chroma plane is too small for libvmaf's multi-scale downsampling, the engine automatically falls back to Y-only scoring instead of failing the task.
    - **False Error Suppression**: Refined stderr parsing to ignore harmless logging fragments (like codec descriptions) that previously triggered false quality verification failures.
- 🧪 **Testing & Debugging Enhancements**: 
    - The `--force` flag now explicitly bypasses the "already modern format" skip logic, allowing users to re-test metadata retention and quality on existing HEVC/AV1 content.

### 📦 Repository & Infrastructure
- **Dependency Bifurcation**: Maintained separation between stable `crates.io` (Main) and edge `GitHub` (Nightly) build targets.
- **Atomic Renaming**: Refined file commitment logic to ensure data remains safe even during sudden power losses or storage exhaustion.

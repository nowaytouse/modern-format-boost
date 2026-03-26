# Modern Format Boost v0.11.0 - Unified Production Consolidation

This release marks a major milestone in the Modern Format Boost project, consolidating the intensive `0.10.x` hardening cycle into a stable, cinema-grade production baseline.

### 🛡️ Cinema-Grade Fidelity (OpenEXR Support)
Unified 8/16/32-bit float intermediate pipeline ensures zero precision loss for HDR and high-dynamic-range sources by leveraging OpenEXR (`.exr`) for intermediate processing.

### 🛠️ Scanner Fortification & Automation
A professional-grade quality scanner (`check_all.sh`) featuring:
- **Parallel Execution Engine** for concurrent `fmt`, `clippy`, and `shellcheck` runs.
- **Automated Quality Fixes** via integrated `cargo fix` and `clippy --fix` loops.
- **High-Precision Diagnostics** with millisecond timing and actionable dependency installation hints.

### 🔍 Intelligent Image Handling
- **Magic Bytes Detection**: Content-aware format identification using the `infer` crate, independent of file extensions.
- **Grayscale ICC Optimization**: Early detection of ICC profile mismatches for immediate fallback routing.
- **Multi-Format Awareness**: Deterministic bit-depth and codec probing for all convertible formats (HEIC, AVIF, WebP, TIFF, BMP, DCRAW).

### ⚡ Search Performance Optimization
- **Smart Sprint Deceleration**: halves search steps near boundaries to discover optimal CRF values that were previously "skipped" by acceleration logic.
- **Resilient Metrics**: Graceful fallback paths for GPU-SSIM and MS-SSIM on unsupported resolutions.

### 🧹 Proactive Housekeeping & Stability
- **Integrated Kondo Cleanup**: Safety-hardened project cleanup that protects Time Machine and Application Data.
- **Zero-Debt Architecture**: 100% clean Clippy (standard/pedantic/nursery) and ShellCheck status.
- **Atomic Integrity**: Hardened temp-file management and checkpoint resilience.

---
*For full details, please refer to the [CHANGELOG.md](https://github.com/nowaytouse/modern-format-boost/blob/main/CHANGELOG.md).*

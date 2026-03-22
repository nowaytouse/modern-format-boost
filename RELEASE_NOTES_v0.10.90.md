### ✨ Features
- 🎞️ **HDR10+ Dynamic Metadata Retention**: Full support for extracting SMPTE 2094-40 metadata via `hdr10plus_tool` and injecting it into x265 outputs via `--dhdr10-info`.
- ⚡ **Perceived Speed Optimization**: "Quick-Wins" sorting strategy—finishes small/shallow files first to provide instant feedback and maximum throughput.
- 🛡️ **Robust Extraction Strategy**: Implemented a "Strict-first, Skip-validation-fallback" strategy for HDR10+ extraction.

### 🛠️ Improvements & Fixes
- 🔄 **Intelligent Checkpoint Reset**: Deleting a manually created output directory (e.g. `_optimized`) now correctly triggers a full re-conversion of the source directory, even in resume mode.
- 🧪 **MS-SSIM/VMAF Quality Verification**: 
    - Fixed false "Pixel format incompatibility" errors by prioritized stdout JSON parsing.
    - Added **Chroma Resolution Guard** (256×256 min) for U/V MS-SSIM channels; gracefully falls back to Y-only scoring instead of crashing.
    - Tightened stderr error matching to avoid false positives from harmless FFmpeg log fragments.
- 🛠️ **Testing Bypass**: Enhanced `--force` flag to explicitly bypass "already modern format" skip logic for easier testing.

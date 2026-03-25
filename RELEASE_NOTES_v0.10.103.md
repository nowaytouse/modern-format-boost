## [0.10.103] - 2026-03-26

### 🐛 Bug Fixes

**Grayscale ICC Early Detection Optimization**

Optimized error handling for JPEG files with mismatched ICC profiles (RGB profile on grayscale image). This edge case affected 2 files out of 12,000 in production testing.

**Problem:**
- Files like `IMG_8321.JPG` contain an ICC profile claiming "RGB color space" but are actually grayscale images
- When `cjxl` internally converts JPEG to PNG, libpng detects the mismatch and fails
- Error pattern: `libpng warning: iCCP: profile 'icc': 'RGB ': RGB color space not permitted on grayscale PNG` followed by `Getting pixel data failed`

**Previous Behavior:**
1. Attempt 1: Direct `cjxl` call → **UPSTREAM ERROR** (failed)
2. System enters FFmpeg fallback pipeline → may also fail
3. Eventually enters ImageMagick fallback pipeline
4. Attempt 2: ImageMagick with `-strip` (removes bad ICC) → **Success**

**Optimized Behavior:**
1. Attempt 1: Direct `cjxl` call → fails
2. **Immediate detection** of grayscale ICC mismatch error
3. **Direct routing** to ImageMagick fallback (skips FFmpeg)
4. Attempt 2: ImageMagick with `-strip` → **Success**

**Benefits:**
- Faster error recovery (eliminates unnecessary FFmpeg pipeline attempt)
- Cleaner log output (reduces noise from failed intermediate attempts)
- More efficient resource usage

**Technical Changes:**
- Made `is_grayscale_icc_cjxl_error()` public in `shared_utils::jxl_utils` for cross-crate reuse
- Added early detection logic in `img_hevc/src/lossless_converter.rs` after first `cjxl` failure
- Routes directly to ImageMagick fallback when grayscale ICC mismatch is detected

**Impact:**
- Affected files: 2 occurrences in 12k image batch
- Both files now process successfully with optimized fallback path
- No changes to final output quality or file handling

---

### 🛠️ Hardening & Technical Debt Cleanup (from 0.10.102)

- **Quality & Performance**:
  - **Zero-Warning Workspace**: Achieved a clean, warning-free build across all crates (`img_hevc`, `img_av1`, `shared_utils`) and shell scripts
  - **Dependency Update**: Full workspace-wide dependency synchronization via `cargo update` to the latest stable and nightly-compatible versions
  
- **Image Intelligence**:
  - **EXR Detection**: Advanced attribute parsing for `OpenEXR` compression types (NONE/RLE/ZIPS/ZIP/PIZ for lossless; DWAA/DWAB etc. for lossy)
  - **JP2 Improvements**: Robust wavelet transform analysis (9/7 irreversible vs 5/3 reversible) via COD/COC marker scanning for precise lossy/lossless detection
  
- **Core Refactoring**:
  - **img_hevc**: Major structural refactoring to align with `img_av1` architecture. Modularized the monolithic conversion logic into specialized dispatch functions, significantly reducing complexity while preserving the advanced video/static logic
  - **img_av1**: Hardened the conversion pipeline with improved error mapping and consistent result reporting
  
- **Shell Script Fortification**: Systematic resolution of all `shellcheck` warnings (SC2155, SC2086, etc.) across the script suite for enhanced reliability

- **Bug Fixes**:
  - **GPU Coarse Search**: Fixed Sprint acceleration logic that was incorrectly resetting after first trigger, now allows continuous step doubling throughout the search phase for improved efficiency
  - **Shell Path Detection**: Fixed `common.sh` to use `${(%):-%x}` for zsh when sourced, preventing incorrect path resolution in multi-script workflows

---

### 📦 Version Synchronization

- Updated all version references to `0.10.103`:
  - Cargo workspace version
  - README.md badge
  - macOS App bundle (CFBundleVersion: 1103, CFBundleShortVersionString: 0.10.103)
  - Documentation references

### 🔧 Maintenance

- Ran `cargo clippy` - all checks passed
- Ran `cargo fmt` - code formatting unified
- Updated dependencies to latest compatible versions
- Synchronized `main` and `nightly` branches (preserving dependency source differences)

---

### Installation

Download pre-compiled binaries from the [Releases](https://github.com/nowaytouse/modern-format-boost/releases/tag/v0.10.103) page.

### Upgrade Notes

This release combines bug fixes and optimizations from v0.10.102 and v0.10.103. No breaking changes or configuration updates required. Simply replace your existing binaries with the new version.

### Compatibility

- Rust: 1.75+
- FFmpeg: 5.0+
- libjxl (cjxl): 0.10+
- ExifTool, ImageMagick, libwebp, dovi_tool, libheif, hdr10plus_tool

---

**Full Changelog**: https://github.com/nowaytouse/modern-format-boost/compare/v0.10.101...v0.10.103


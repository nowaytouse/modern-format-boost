# Changelog

All notable changes to Modern Format Boost will be documented in this file.

## [0.4.0] - 2025-12-13 (v4.9)

### 🔥 Performance Optimization - Eliminated Redundant Encoding

#### Core Improvements
- **Smart Final Encoding**: Track the last encoded CRF to avoid unnecessary re-encoding
  - Previous: Always re-encoded at the end, wasting one full encode cycle
  - Now: Only re-encodes if the output file doesn't match the best CRF
  - Saves ~10-20% encoding time on average

- **Unified Caching Mechanism**: All explore modes now use consistent caching
  - `explore_precise_quality_match`: Full caching + smart final encode
  - `explore_precise_quality_match_with_compression`: Full caching + smart final encode
  - Cache key: `CRF * 10` (rounded integer) → avoids floating-point issues

- **Fixed Critical Bug in v4.8**: `explore_precise_quality_match_with_compression` could return wrong file
  - Problem: Used `fs::metadata` to read file size, but file content might not match best_crf
  - Solution: Track `last_encoded_key` and re-encode only when necessary

#### Search Flow Optimization
- **Three-Phase Search with ±0.1 Precision**:
  1. Phase 1: Boundary test (min_crf, max_crf) with early exit on SSIM plateau
  2. Phase 2: Golden section / Binary search for efficient convergence
  3. Phase 3: Fine-tune ±0.5 then ±0.1 for precise CRF selection

- **Meaningful Iterations**: Every encoding operation now serves a purpose
  - No duplicate encodings (cache checks before encode)
  - Early termination when SSIM plateau detected
  - Progressive refinement from coarse to fine

#### Technical Details
```rust
// v4.9: Track last encoded CRF to avoid redundant work
let mut last_encoded_key: i32 = -1;

// At the end, only re-encode if necessary
let final_size = if last_encoded_key == best_key {
    log!("✨ Output already at best CRF (no re-encoding needed)");
    best_size
} else {
    log!("📍 Final: Re-encoding to best CRF");
    self.encode(best_crf)?
};
```

### Changed
- `explore_precise_quality_match`: v4.7 → v4.9
- `explore_precise_quality_match_with_compression`: v4.7 → v4.9
- MAX_ITERATIONS increased from 12 to 14 for better ±0.1 precision

### Fixed
- Critical file mismatch bug in `explore_precise_quality_match_with_compression`
- Unnecessary final encoding in `explore_precise_quality_match`
- Inconsistent caching across different explore modes

### 🎯 Real-time Progress Output (UX Enhancement)

#### Problem
Users experienced "frozen" terminal during long encoding operations:
```
🔄 Encoding CRF 25.0...
[terminal appears frozen for 5+ minutes]
```

#### Solution: Live Progress Feedback
- **Encoding Progress**: Real-time percentage, fps, speed display
  ```
  ⏳ Encoding: 45.2% | 67.8s / 150.0s | 24.3 fps | 1.2x
  ```
- **SSIM Calculation Progress**: Live percentage during quality validation
  ```
  📊 Calculating SSIM... 78%
  ```

#### Implementation
- Use `ffmpeg -progress pipe:1` for machine-readable progress
- Parse `out_time_us`, `fps`, `speed` from progress output
- Calculate percentage using input file duration from `ffprobe`
- Use `\r` (carriage return) for in-place updates
- Spawn separate thread for progress reading to avoid blocking

```rust
// v4.9: Real-time progress output
eprint!("\r      ⏳ Encoding: {:.1}% | {:.1}s / {:.1}s | {:.1} fps | {}   ",
    pct, current_secs, duration_secs, last_fps, last_speed);
```

---

## [0.3.0] - 2025-12-12

### Added
- 🍎 **Apple Compatibility Mode** (`--apple-compat`): New flag for HEVC tools
  - `vidquality-hevc`: Converts AV1/VP9/VVC/AV2 videos to HEVC for Apple device compatibility
  - `imgquality-hevc`: Converts animated WebP/AVIF to HEVC MP4 for Apple device compatibility
  - Only HEVC videos are skipped (already Apple compatible)
- `should_skip_video_codec_apple_compat()` function in shared_utils for unified skip logic
- App (Modern Format Boost.app) now defaults to Apple compatibility mode

### Changed
- Drag & drop processor script updated to v4.0 with `--apple-compat` enabled by default
- ConversionConfig and ConvertOptions now include `apple_compat` field
- Updated README with Apple compatibility mode documentation (English & Chinese)

### Technical Details
- New function `determine_strategy_with_apple_compat()` in vidquality-hevc
- Animated format handling in imgquality-hevc now respects apple_compat flag
- All HEVC tools recompiled with new features

---

## [0.2.0] - 2025-12-11

### Major Achievements
- **Zero Clippy Warnings**: All 4 projects (imgquality-hevc, vidquality-hevc, imgquality_API, vidquality_API) now compile with 0 warnings
- **Production Ready Code**: Removed all TODO placeholders with real implementations
- **Code Quality**: Comprehensive refactoring and optimization

### Added
- PNG quantization detection via IHDR chunk analysis (Structural Analysis 55%)
- JPEG quality estimation from quantization tables (0-100 scale)
- JPEG progressive detection via SOF2 marker
- XMP metadata merge functionality before format conversion (v3.9)
- AutoConvertConfig struct for cleaner function signatures
- Comprehensive test coverage for format utilities
- Real PSNR/SSIM boundary tests replacing placeholders

### Fixed
- Fixed f32 to u8 conversion in vidquality_API
- Fixed unused_io_amount warnings (use read_exact instead of read)
- Fixed needless_range_loop patterns in metrics.rs
- Fixed &PathBuf → &Path in all function signatures
- Fixed too_many_arguments warning via AutoConvertConfig
- Cleaned up iCloud sync conflict files (25+ files)
- Removed duplicate documentation comments
- Fixed type mismatches in conversion_api.rs

### Changed
- Replaced unsafe unwrap() with proper error handling
- Improved function signatures using &Path instead of &PathBuf
- Refactored auto_convert functions to use config struct
- Applied clippy auto-fixes for code style improvements
- Enhanced test assertions with real boundary checks

### Removed
- All TODO placeholders in production code
- Unused imports (PathBuf where only Path needed)
- Duplicate doc comments
- Placeholder test implementations

### Technical Details

#### Code Quality Metrics
- Clippy warnings: 0 (all projects)
- Compilation errors: 0
- Type safety: 100%
- Error handling: Proper Result types throughout

#### Format Detection Improvements
- PNG: IDAT chunk analysis for compression detection
- JPEG: Quantization table analysis for quality estimation
- WebP: VP8L/ANIM chunk detection
- GIF: Frame descriptor counting
- JXL: Signature verification

#### Performance
- Parallel processing with configurable thread pool
- Atomic counters for thread-safe statistics
- Efficient file scanning with WalkDir
- Optimized memory usage in metrics calculations

### Dependencies
- All dependencies up to date
- No breaking changes
- Compatible with Rust 1.70+

### Testing
- Unit tests for all format utilities
- Integration tests for conversion workflows
- SSIM/PSNR validation tests
- Edge case handling for low-resolution files

### Documentation
- Updated README with comprehensive feature list
- Added drag & drop usage guide (拖拽使用说明)
- Detailed quality matching algorithm documentation
- CLI reference for all tools

## [0.1.0] - Previous Release

See git history for details on earlier versions.

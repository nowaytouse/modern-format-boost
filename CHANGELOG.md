# Changelog

All notable changes to Modern Format Boost will be documented in this file.

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

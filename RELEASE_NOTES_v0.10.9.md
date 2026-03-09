# Release Notes - v0.10.9

## Critical Bug Fixes

### FFprobe Image2 Demuxer Pattern Matching Issue
Fixed critical bug where image files with `[` `]` characters in filenames failed to process due to FFprobe's image2 demuxer interpreting them as sequence patterns.

**Example**: File `FB55N[I_R{KE)K}I141L%8V.jpeg` would fail with error:
```
[image2 @ 0x...] Could find no file with path 'FB55N[I_R{KE)K}I141L%8V.jpeg' and index in the range 0-4
```

**Solution**: Added automatic fallback with `-pattern_type none` parameter when pattern matching error is detected. The fix:
1. Attempts normal ffprobe call
2. Detects image2 demuxer pattern error in stderr
3. Automatically retries with `-pattern_type none` to disable sequence pattern matching

**Impact**: All image files with special characters (`[`, `]`, `{`, `}`, `%`) in names can now be processed correctly.

### Silent FFprobe Errors
Fixed bug where ffprobe errors were silently suppressed due to `-v quiet` flag, preventing proper error detection and fallback handling.

**Solution**: Changed all ffprobe calls from `-v quiet` to `-v error` to capture error messages while still suppressing info/warning output.

**Impact**: Better error diagnostics and proper fallback behavior for edge cases.

### Missing Success Output
Fixed bug where successful conversions showed no output unless `--verbose` flag was used, leaving users uncertain whether conversion succeeded.

**Before**:
```bash
$ img-hevc run --output ./out image.jpg
# (no output)
```

**After**:
```bash
$ img-hevc run --output ./out image.jpg
✅ JPEG lossless transcode conversion successful: size reduced 25.5%
```

**Impact**: Users now see clear feedback when conversions succeed.

## Quality Improvements

### Misleading Quality Check Messages
Fixed logical paradox in quality verification messages where logs showed mathematically impossible comparisons like "0.9939 < 0.90".

**Root Cause**: In Ultimate Mode, `ms_ssim_score` stores VMAF-Y (0-1 scale), not MS-SSIM. Quality gate can fail due to CAMBI (banding) or PSNR-UV (chroma) even with high VMAF (99.39%).

**Solution**: Changed messages to generic "QUALITY TARGET FAILED (score: X.XXXX)" without misleading comparison operators.

### Timestamp Verification Diagnostics
Improved error handling for filesystem timestamp sync failures with better diagnostic messages.

**Before**:
```
⚠️ Failed to restore directory timestamps for /path/to/dir: Permission denied
```

**After**:
```
⚠️ Failed to restore directory timestamp for /path/to/dir: Permission denied
⚠️ TIMESTAMP VERIFICATION: 3/10 directories failed (possible filesystem protection or network mount)
```

**Impact**: Users understand this is expected behavior on protected filesystems or network mounts, not a program bug.

## Previous Fixes (v0.10.8 - v0.10.9)

### FFprobe Special Characters in Filenames
Added `--` separator before file path arguments in all ffprobe invocations to prevent interpretation as options/patterns. This fixed issues with files containing special characters in their names.

### x265 Calibration Empty Y4M Guard
Added file size validation after ffmpeg y4m extraction to prevent x265 from receiving empty files. When ffmpeg exits with code 0 but writes 0 bytes (e.g., no decodable frames in first 15 seconds), the calibration attempt is skipped with clear diagnostic message instead of misleading x265 error.

## Technical Details

### Files Modified
- `shared_utils/src/ffprobe_json.rs` - Image2 demuxer fallback + error level change
- `shared_utils/src/image_analyzer.rs` - Error level change for ffprobe
- `img_hevc/src/main.rs` - Always show success messages
- `img_av1/src/main.rs` - Always show success messages
- `vid_hevc/src/conversion_api.rs` - Quality check message improvements
- `shared_utils/src/metadata/mod.rs` - Timestamp verification diagnostics
- `shared_utils/src/video_explorer/dynamic_mapping.rs` - Y4M empty file guard

### Dependencies Updated
- `zerocopy`: 0.8.40 → 0.8.41
- `zerocopy-derive`: 0.8.40 → 0.8.41

## Testing

All fixes have been tested with actual problematic files:
- ✅ `FB55N[I_R{KE)K}I141L%8V.jpeg` - Special characters in filename
- ✅ `6946418393937362319.mp4` - Empty y4m calibration issue
- ✅ Success output now displays correctly
- ✅ All unit tests passing (805 tests)

## Installation

Download the appropriate binary for your platform from the release assets below.

### macOS
```bash
# ARM64 (Apple Silicon)
curl -L https://github.com/nowaytouse/modern-format-boost/releases/download/v0.10.9/modern-format-boost-macos-arm64.tar.gz | tar xz

# x86_64 (Intel)
curl -L https://github.com/nowaytouse/modern-format-boost/releases/download/v0.10.9/modern-format-boost-macos-x64.tar.gz | tar xz
```

### Linux
```bash
curl -L https://github.com/nowaytouse/modern-format-boost/releases/download/v0.10.9/modern-format-boost-linux-x86_64.tar.gz | tar xz
```

### Windows
Download `modern-format-boost-windows-x64.zip` from the release assets and extract.

## Upgrade Notes

This is a bug fix release with no breaking changes. All existing command-line arguments and configurations remain compatible.

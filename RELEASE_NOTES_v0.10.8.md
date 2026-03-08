# Release v0.10.8 - Multi-Stream Animated Image Fix

## Critical Bug Fixes

### 🐛 Multi-Stream AVIF/HEIC Stream Selection Bug
Fixed a critical bug where animated AVIF/HEIC files with multiple video streams (thumbnail + animation) would only convert the first frame instead of all frames.

**Root Cause**: `probe_video()` was returning the enumerate array index (0, 1, 2...) instead of the actual stream index from ffprobe JSON.

**Impact**: 
- Animated AVIF files with 3 frames would only output 1 frame
- Multi-stream HEIC files were similarly affected
- GBR colorspace AVIF files were affected

**Fix**:
- Modified `probe_video()` to extract actual stream `index` field from JSON
- Added multi-stream detection in `convert_to_hevc_mp4_matched()`
- Automatically convert multi-stream AVIF/HEIC to APNG before processing
- APNG preserves all frames and timing information

**Files Modified**:
- `shared_utils/src/ffprobe.rs` - Fixed stream index extraction
- `vid_hevc/src/animated_image.rs` - Added multi-stream APNG conversion

### ✅ Verified Test Results

**AVIF GBR (3 frames, multi-stream)**:
- ✅ → MOV: 3 frames, 0.3s, 10fps, HEVC, YUV420p (colorspace correctly converted)
- ✅ → GIF: 3 frames, 0.3s, 10fps

**AVIF YUV (3 frames, multi-stream)**:
- ✅ → MOV: 3 frames, 0.3s, 10fps, HEVC, YUV420p

**WebP (3 frames)**:
- ✅ → MOV: 3 frames, 0.3s, 10fps, HEVC
- ✅ → GIF: 3 frames, 0.3s, 10fps

## Previous Fixes (v0.10.7)

### 🔧 WebP Frame Extraction and Timing
Complete rewrite of WebP → video conversion pipeline using `webpmux` for accurate frame extraction.

**Problem**: ImageMagick's WebP → APNG conversion was unreliable (frame duplication, incorrect timing).

**Solution**:
1. Use `webpmux -info` to get accurate frame count and duration from WebP metadata
2. Use `webpmux -get frame N` to extract each frame as WebP
3. Convert each WebP frame to PNG using FFmpeg
4. Create APNG from PNG sequence with correct frame rate

**Requirement**: `webpmux` tool must be installed (part of libwebp package)

### 🔧 APNG Duration Detection
Fixed ffprobe inability to read APNG duration metadata.

**Solution**:
- Added `-count_frames` parameter to ffprobe
- Use `nb_read_frames` for frame count when `nb_frames` is unavailable
- Calculate duration from frame count and fps

## Code Quality

### ✅ Clippy Clean
All code passes `cargo clippy -- -D warnings` with no warnings.

### ✅ No Hardcoding or Workarounds
- All frame counts are dynamically parsed from actual file metadata
- All durations are calculated from actual frame rates
- Stream selection uses actual ffprobe data
- No magic numbers or hardcoded values for frame counts/durations

## Technical Details

### Stream Selection Algorithm
```rust
// Select stream with most frames (for animated images)
video_streams.iter()
    .max_by_key(|(_, s)| {
        s["nb_frames"]
            .as_str()
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0)
    })
    .map(|(_, s)| {
        let actual_index = s["index"].as_u64().unwrap_or(0) as usize;
        (actual_index, *s)
    })
```

### Multi-Stream AVIF/HEIC Processing
For files with multiple video streams:
1. Detect multiple streams using ffprobe
2. Select stream with most frames (animation, not thumbnail)
3. Convert selected stream to APNG using FFmpeg
4. Process APNG through normal pipeline
5. Temporary files automatically cleaned up

## Installation

### Requirements
- FFmpeg with HEVC support
- `webpmux` (for WebP processing): `brew install webp` or `apt-get install webp`
- `djxl` (for JXL processing): `brew install jpeg-xl` or `apt-get install libjxl-tools`

### Build from Source
```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release
```

Binaries will be in `target/release/`:
- `img-hevc` - Image and animated image converter
- `vid-hevc` - Video converter

## Usage Examples

### Convert Animated AVIF to MOV
```bash
img-hevc run input.avif --apple-compat --force-video
```

### Convert Animated WebP to GIF
```bash
img-hevc run input.webp --apple-compat
```

### Batch Process Directory
```bash
img-hevc run /path/to/images --recursive --apple-compat
```

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for complete version history.

## Contributors

Thanks to all contributors who helped identify and fix these issues!

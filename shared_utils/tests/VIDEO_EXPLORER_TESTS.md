# Test Media Infrastructure

This directory contains test fixtures and integration tests for the video_explorer module.

## Directory Structure

```
test/
├── generate_test_media.sh          # Script to generate all test media
├── MEDIA_MANIFEST.md               # Documentation of test media files
├── videos/                         # Test video files
│   ├── test_h264_10s.mp4
│   ├── test_vp9_5s.webm
│   ├── test_hevc_8s.mp4
│   ├── test_av1_6s.mkv
│   ├── test_hq_source_15s.mp4
│   ├── test_lq_source_12s.mp4
│   └── test_short_2s.mp4
├── images/                         # Test image files (optional)
├── gifs/                          # Test GIF files
│   ├── test_simple.gif
│   └── test_pattern.gif
└── video_explorer_tests/          # Integration tests
    └── integration_tests.rs
```

## Test Media Files

All test media is **synthetically generated** for reproducibility:

### Video Files

| File | Codec | Duration | Resolution | CRF | Use Case |
|------|-------|----------|------------|-----|----------|
| test_h264_10s.mp4 | H.264 | 10s | 1280×720 | 23 | Baseline H.264 testing, CRF [10-28] |
| test_vp9_5s.webm | VP9 | 5s | 1920×1080 | 28 | VP9→HEVC/AV1 conversion |
| test_hevc_8s.mp4 | HEVC | 8s | 1920×1080 | 28 | HEVC codec testing |
| test_av1_6s.mkv | AV1 | 6s | 1920×1080 | 30 | AV1 codec testing, CRF [10-35] |
| test_hq_source_15s.mp4 | H.264 | 15s | 1920×1080 | 18 | High-quality source (SSIM ~0.98) |
| test_lq_source_12s.mp4 | H.264 | 12s | 640×480 | 35 | Low-quality source (SSIM ~0.85) |
| test_short_2s.mp4 | H.264 | 2s | 1280×720 | 23 | Quick tests, duration fallback |

## Running Tests

### 1. Generate Test Media

```bash
cd test
./generate_test_media.sh
```

**Requirements:**
- ffmpeg with support for: libx264, libx265, libvpx-vp9, libaom-av1, libopus

**Time:** ~2-3 minutes (first run only)

**Output:** All media files (~30-50 MB)

### 2. Run Unit Tests with Media

```bash
# Run all video_explorer tests
cargo test --lib video_explorer

# Run specific test category
cargo test --lib video_explorer::tests::test_precision_crf_search_range

# Run property-based tests
cargo test --lib video_explorer::prop_tests_v69

# Run integration tests (requires media files)
cargo test --test integration_tests
```

### 3. Verify Media Availability

```bash
# Quick check that all media files exist
cargo test integration_tests::media_files_exist

# Detailed media validation
cargo test integration_tests::test_all_codec_variants_available
```

## Test Coverage

### Unit Tests (52 tests)
- ✅ CRF precision calculations
- ✅ SSIM quality thresholds
- ✅ Binary search optimization
- ✅ Algorithm constants
- ✅ Quality validation logic

### Property-Based Tests (3 tests)
- ✅ Duration fallback calculation
- ✅ Zero-gains minimum guarantee
- ✅ Zero-gains scaling behavior

### Integration Tests (Media-dependent)
- ✅ Media file presence validation
- ✅ Codec variant availability
- ✅ Quality range testing (HQ/LQ sources)
- ✅ CRF precision with real videos
- ✅ SSIM quality grades with sources

## Test Media Specifications

### CRF Precision Tests

The video_explorer module tests CRF precision for three ranges:

**HEVC Range: [10, 28]**
- Video: test_h264_10s.mp4
- Expected iterations: 6 (≤8 maximum)
- Step sizes: Coarse 2.0, Fine 0.5

**AV1 Range: [10, 35]**
- Video: test_av1_6s.mkv
- Expected iterations: 6 (≤8 maximum)
- Step sizes: Coarse 2.5, Fine 0.5

**Wide Range: [0, 51]**
- Video: test_hq_source_15s.mp4
- Expected iterations: 7 (≤8 maximum)
- Step sizes: Coarse 2.5, Fine 0.5

### SSIM Quality Testing

**Excellent Quality (≥0.97):** test_hq_source_15s.mp4 (CRF 18)
**Good Quality (≥0.95):** test_h264_10s.mp4 (CRF 23)
**Acceptable Quality (≥0.90):** test_lq_source_12s.mp4 (CRF 35)
**Fair Quality (≥0.85):** test_vp9_5s.webm (CRF 28)

### Zero-Gains Validation

Tests use videos of varying durations:
- Short: 2 seconds (test_short_2s.mp4)
- Medium: 10 seconds (test_h264_10s.mp4)
- Long: 15 seconds (test_hq_source_15s.mp4)

**Expected behavior:**
- Normal mode: minimum 3 iterations
- Ultimate mode: minimum 15 iterations
- Scales with CRF range and duration

## Adding New Test Media

To add additional test videos:

1. **Edit generate_test_media.sh** with new ffmpeg commands
2. **Update MEDIA_MANIFEST.md** with specifications
3. **Add integration tests** in integration_tests.rs
4. **Regenerate media:** `./generate_test_media.sh`

### Example: Add H.265 Main 10 (10-bit) Video

```bash
# In generate_test_media.sh, add:
ffmpeg -f lavfi -i "color=c=purple:s=1920x1080:d=10" \
  -f lavfi -i "sine=f=550:d=10" \
  -c:v libx265 -preset fast -crf 28 -profile:v main10 \
  -c:a aac -b:a 128k \
  -y "$VIDEOS_DIR/test_hevc_main10_10s.mp4" 2>/dev/null || true
```

Then add corresponding test:
```rust
#[test]
fn test_hevc_main10_profile_available() {
    assert!(video_exists("test_hevc_main10_10s.mp4"));
}
```

## CI/CD Integration

### GitHub Actions Example

```yaml
- name: Generate Test Media
  run: bash test/generate_test_media.sh

- name: Run All Tests
  run: cargo test

- name: Run Integration Tests
  run: cargo test --test integration_tests
```

## Performance Notes

- **Media generation:** ~2-3 minutes (one-time)
- **Unit tests:** ~10 seconds
- **Integration tests:** ~5 seconds (media files already present)
- **Total test suite:** ~15 seconds

## Troubleshooting

### "ffmpeg command not found"
```bash
# macOS
brew install ffmpeg

# Linux (Ubuntu/Debian)
sudo apt-get install ffmpeg

# Linux (Fedora/RHEL)
sudo dnf install ffmpeg
```

### "libaom-av1 not found"
```bash
# Rebuild ffmpeg with AV1 support
brew reinstall ffmpeg --with-libaom

# Or use pre-built binary with AV1:
https://ffmpeg.org/download.html
```

### Test files corrupted or missing
```bash
# Regenerate all media
rm test/videos/*.* test/gifs/*.*
bash test/generate_test_media.sh
```

## References

- [SSIM (Structural Similarity Index)](https://en.wikipedia.org/wiki/Structural_similarity)
- [CRF (Constant Rate Factor)](https://trac.ffmpeg.org/wiki/Encode/H.264)
- [VP9 Codec](https://en.wikipedia.org/wiki/VP9)
- [AV1 Codec](https://en.wikipedia.org/wiki/AV1)
- [HEVC/H.265](https://en.wikipedia.org/wiki/High_Efficiency_Video_Coding)

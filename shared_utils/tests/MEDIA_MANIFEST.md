# Test Media Manifest

This directory contains synthetic test media files for unit testing the video_explorer module.

## Images

### test_image_1080p.png
- Resolution: 1920x1080
- Format: PNG (lossless)
- Color: Blue
- Use: Basic image testing

### test_gradient_red.png
- Resolution: 800x600
- Format: PNG (lossless)
- Color: Red gradient
- Use: Color space testing

### test_hd_4k.png
- Resolution: 3840x2160 (4K)
- Format: PNG (lossless)
- Color: Green
- Use: High-resolution testing

### test_low_quality.png
- Resolution: 640x480 (VGA)
- Format: PNG (lossless)
- Color: Yellow
- Use: Low-resolution quality ceiling testing

## Videos

### test_h264_10s.mp4
- Codec: H.264 (AVC)
- Duration: 10 seconds
- Resolution: 1280x720
- Bitrate: ~500 kbps (adaptive)
- CRF: 23 (moderate quality)
- Use: Baseline conversion testing, CRF range [10-28]

### test_vp9_5s.webm
- Codec: VP9
- Duration: 5 seconds
- Resolution: 1920x1080
- Bitrate: ~1000 kbps (adaptive)
- CRF: 28 (good quality)
- Use: VP9 to HEVC/AV1 conversion testing

### test_hevc_8s.mp4
- Codec: HEVC (H.265)
- Duration: 8 seconds
- Resolution: 1920x1080
- Bitrate: ~800 kbps (adaptive)
- CRF: 28 (good quality)
- Use: HEVC codec testing, skip validation

### test_av1_6s.mkv
- Codec: AV1
- Duration: 6 seconds
- Resolution: 1920x1080
- Bitrate: ~600 kbps (adaptive)
- CRF: 30 (good quality)
- Use: AV1 codec testing, CRF range [10-35]

### test_hq_source_15s.mp4
- Codec: H.264 (high quality)
- Duration: 15 seconds
- Resolution: 1920x1080
- Bitrate: ~2000 kbps (adaptive)
- CRF: 18 (high quality source)
- Use: High-quality source handling, SSIM calibration

### test_lq_source_12s.mp4
- Codec: H.264 (low quality)
- Duration: 12 seconds
- Resolution: 640x480
- Bitrate: ~200 kbps (adaptive)
- CRF: 35 (low quality source)
- Use: Low-quality source ceiling testing, zero-gains validation

### test_short_2s.mp4
- Codec: H.264
- Duration: 2 seconds
- Resolution: 1280x720
- Bitrate: ~300 kbps (adaptive)
- CRF: 23 (moderate quality)
- Use: Quick integration tests, duration fallback testing

## GIFs

### test_simple.gif
- Duration: 2 seconds
- Resolution: 640x480
- Frame rate: 10 fps
- Use: GIF detection and handling

### test_pattern.gif
- Duration: 3 seconds
- Resolution: 320x240
- Frame rate: 10 fps
- Colors: Test pattern (various)
- Use: Complex GIF pattern testing

## Test Categories Supported

### CRF Precision Tests
- **Supported files**: test_h264_10s.mp4, test_av1_6s.mkv, test_hevc_8s.mp4
- **CRF ranges**: HEVC [10-28], AV1 [10-35], Wide [0-51]
- **Expected iterations**: HEVC ≤8, AV1 ≤8, Wide ≤8

### SSIM Quality Tests
- **Supported files**: test_hq_source_15s.mp4, test_lq_source_12s.mp4
- **Thresholds**: Excellent (0.97+), Good (0.95+), Acceptable (0.90+), Fair (0.85+), Poor (<0.85)

### Source Quality Handling
- **High quality**: test_hq_source_15s.mp4 (CRF 18, SSIM ~0.98)
- **Low quality**: test_lq_source_12s.mp4 (CRF 35, SSIM ~0.85)

### Zero-Gains Validation
- **Duration ranges**: 2s to 15s
- **CRF ranges**: 1.0 to 50.0
- **Expected minimum**: 3 iterations (normal), 15 iterations (ultimate mode)

### Property-Based Testing (proptest)
- **Duration fallback**: test_short_2s.mp4 (2s), test_hq_source_15s.mp4 (15s)
- **Zero-gains scaling**: test_lq_source_12s.mp4 (low CRF), test_av1_6s.mkv (high CRF)
- **Cases per test**: 100 (configurable in proptest config)

## Generation Command

To regenerate all test media:

```bash
./test/generate_test_media.sh
```

## Storage Considerations

- Total size: ~30-50 MB (depends on codec efficiency)
- Recommended disk space: 100 MB
- All files are synthetic (color/pattern based) for quick generation
- For production testing, consider adding real-world media samples

## Notes

- Files are generated with `ffmpeg` - ensure it's installed
- Synthetic media (colored frames + sine wave audio) ensures reproducible, fast generation
- All durations and specifications are optimized for unit test execution speed
- No external media files needed for basic test coverage

#!/usr/bin/env python3
"""Generate test media for video_explorer unit tests.

This script creates synthetic media files needed for testing.
"""

import subprocess
import sys
from pathlib import Path


def run_ffmpeg(args: list[str], output_file: str) -> bool:
    """Run ffmpeg command and return success status."""
    try:
        subprocess.run(
            ["ffmpeg"] + args + ["-y", output_file],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=True,
        )
        return True
    except (subprocess.CalledProcessError, FileNotFoundError):
        return False


def main() -> int:
    """Generate all test media files."""
    script_dir = Path(__file__).parent.resolve()
    test_dir = script_dir
    images_dir = test_dir / "images"
    videos_dir = test_dir / "videos"
    gifs_dir = test_dir / "gifs"

    # Create directories
    images_dir.mkdir(parents=True, exist_ok=True)
    videos_dir.mkdir(parents=True, exist_ok=True)
    gifs_dir.mkdir(parents=True, exist_ok=True)

    print(f"🎬 Generating test media in {test_dir}...")

    # ========================================================================
    # IMAGE GENERATION
    # ========================================================================
    print("📸 Generating test images...")

    images = [
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=1920x1080:d=1",
                "-f",
                "lavfi",
                "-i",
                "sine=f=1000:d=1",
                "-c:v",
                "png",
                "-c:a",
                "pcm_s16le",
            ],
            str(images_dir / "test_image_1080p.png"),
        ),
        (
            ["-f", "lavfi", "-i", "color=red:s=800x600:d=1", "-c:v", "png"],
            str(images_dir / "test_gradient_red.png"),
        ),
        (
            ["-f", "lavfi", "-i", "color=green:s=3840x2160:d=1", "-c:v", "png"],
            str(images_dir / "test_hd_4k.png"),
        ),
        (
            ["-f", "lavfi", "-i", "color=yellow:s=640x480:d=1", "-c:v", "png"],
            str(images_dir / "test_low_quality.png"),
        ),
    ]

    for args, output in images:
        run_ffmpeg(args, output)

    # ========================================================================
    # VIDEO GENERATION
    # ========================================================================
    print("🎥 Generating test videos...")

    videos = [
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=1280x720:d=10",
                "-f",
                "lavfi",
                "-i",
                "sine=f=440:d=10",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "23",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
            ],
            str(videos_dir / "test_h264_10s.mp4"),
        ),
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=green:s=1920x1080:d=5",
                "-f",
                "lavfi",
                "-i",
                "sine=f=880:d=5",
                "-c:v",
                "libvpx-vp9",
                "-preset",
                "fast",
                "-crf",
                "28",
                "-c:a",
                "libopus",
                "-b:a",
                "128k",
            ],
            str(videos_dir / "test_vp9_5s.webm"),
        ),
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=1920x1080:d=8",
                "-f",
                "lavfi",
                "-i",
                "sine=f=660:d=8",
                "-c:v",
                "libx265",
                "-preset",
                "fast",
                "-crf",
                "28",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
            ],
            str(videos_dir / "test_hevc_8s.mp4"),
        ),
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=yellow:s=1920x1080:d=6",
                "-f",
                "lavfi",
                "-i",
                "sine=f=1000:d=6",
                "-c:v",
                "libaom-av1",
                "-preset",
                "4",
                "-crf",
                "30",
                "-c:a",
                "libopus",
                "-b:a",
                "128k",
            ],
            str(videos_dir / "test_av1_6s.mkv"),
        ),
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=cyan:s=1920x1080:d=15",
                "-f",
                "lavfi",
                "-i",
                "sine=f=1200:d=15",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "18",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
            ],
            str(videos_dir / "test_hq_source_15s.mp4"),
        ),
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=magenta:s=640x480:d=12",
                "-f",
                "lavfi",
                "-i",
                "sine=f=500:d=12",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "35",
                "-c:a",
                "aac",
                "-b:a",
                "64k",
            ],
            str(videos_dir / "test_lq_source_12s.mp4"),
        ),
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=white:s=1280x720:d=2",
                "-f",
                "lavfi",
                "-i",
                "sine=f=800:d=2",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "23",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
            ],
            str(videos_dir / "test_short_2s.mp4"),
        ),
    ]

    for args, output in videos:
        run_ffmpeg(args, output)

    # ========================================================================
    # GIF GENERATION
    # ========================================================================
    print("🎬 Generating test GIFs...")

    gifs = [
        (
            [
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=640x480:d=2",
                "-f",
                "lavfi",
                "-i",
                "sine=f=440:d=2",
                "-vf",
                "fps=10,scale=640:480:flags=lanczos",
            ],
            str(gifs_dir / "test_simple.gif"),
        ),
        (
            ["-f", "lavfi", "-i", "testsrc=s=320x240:d=3", "-vf", "fps=10"],
            str(gifs_dir / "test_pattern.gif"),
        ),
    ]

    for args, output in gifs:
        run_ffmpeg(args, output)

    # ========================================================================
    # MEDIA SPECIFICATIONS
    # ========================================================================
    manifest_content = """# Test Media Manifest

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
python3 generate_test_media.py
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
"""

    manifest_path = test_dir / "MEDIA_MANIFEST.md"
    manifest_path.write_text(manifest_content)
    print(f"✅ Media manifest created at {manifest_path}")

    # ========================================================================
    # SUMMARY
    # ========================================================================
    print()
    print("=" * 64)
    print("✅ Test media generation complete!")
    print("=" * 64)
    print()
    print("Generated media files:")
    print(f"📁 Images:  {len(list(images_dir.iterdir()))} files")
    print(f"📁 Videos:  {len(list(videos_dir.iterdir()))} files")
    print(f"📁 GIFs:    {len(list(gifs_dir.iterdir()))} files")
    print()
    print(f"For test specifications, see: {manifest_path}")
    print("=" * 64)

    return 0


if __name__ == "__main__":
    sys.exit(main())

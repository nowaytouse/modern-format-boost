#!/usr/bin/env python3
"""Generate test media for video_explorer unit tests.

This script creates synthetic media files needed for testing.
"""

from mfb_ui_tokens import pick_symbol
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
    # SSOT for integration tests: crates/dev/src/tests/edge (not scripts/).
    test_dir = script_dir.parent / "tests" / "edge"
    images_dir = test_dir / "images"
    videos_dir = test_dir / "videos"
    gifs_dir = test_dir / "gifs"

    # Create directories
    images_dir.mkdir(parents=True, exist_ok=True)
    videos_dir.mkdir(parents=True, exist_ok=True)
    gifs_dir.mkdir(parents=True, exist_ok=True)

    print(f"{pick_symbol('🎬', ('[VID]'))} Generating test media in {test_dir}...")

    # ========================================================================
    # IMAGE GENERATION
    # ========================================================================
    print(f"{pick_symbol('📸', ('[IMG]'))} Generating test images...")

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
    print(f"{pick_symbol('🎥', ('[VID]'))} Generating test videos...")

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
    print(f"{pick_symbol('🎬', ('[VID]'))} Generating test GIFs...")

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

    # tests/edge/MEDIA_MANIFEST.md is hand-maintained (extra poison-pill assets, etc.).

    # ========================================================================
    # SUMMARY
    # ========================================================================
    print()
    print("=" * 64)
    print(f"{pick_symbol('✅', ('[OK]'))} Test media generation complete!")
    print("=" * 64)
    print()
    print("Generated media files:")
    print(
        f"{pick_symbol('📁', ('[DIR]'))} Images:  {len(list(images_dir.iterdir()))} files"
    )
    print(
        f"{pick_symbol('📁', ('[DIR]'))} Videos:  {len(list(videos_dir.iterdir()))} files"
    )
    print(
        f"{pick_symbol('📁', ('[DIR]'))} GIFs:    {len(list(gifs_dir.iterdir()))} files"
    )
    print()
    manifest_path = test_dir / "MEDIA_MANIFEST.md"
    if manifest_path.is_file():
        print(f"For test specifications, see: {manifest_path}")
    print("=" * 64)

    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""
Convert video to Apple Live Photo format (IMG_xxxx.JPG/HEIC + IMG_xxxx.MOV).
Supports high-quality encoding, HEIC format, and Live Photo metadata injection.
"""

import argparse
import shutil
import subprocess
import sys
from datetime import datetime
from pathlib import Path

from mfb_ui_tokens import pick_symbol


def check_dependencies(needs_heif=False, needs_makelive=False):
    """Check required dependency tools"""
    deps = ["ffmpeg", "ffprobe"]
    if needs_heif:
        deps.append("heif-enc")
    if needs_makelive:
        deps.append("makelive")

    missing = []
    for dep in deps:
        if shutil.which(dep) is None:
            missing.append(dep)

    if missing:
        print(f"Error: Missing required dependencies: {', '.join(missing)}")
        if "heif-enc" in missing:
            print("Hint: Install heif-enc via 'brew install libheif'")
        if "makelive" in missing:
            print("Hint: Install makelive or related tools")
        sys.exit(1)


def get_video_info(video_path):
    """Get video duration and resolution"""
    try:
        # Duration
        probe_cmd = [
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            str(video_path),
        ]
        duration = float(subprocess.check_output(probe_cmd).decode().strip())

        # Resolution
        res_probe = [
            "ffprobe",
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            str(video_path),
        ]
        resolution = subprocess.check_output(res_probe).decode().strip()

        return duration, resolution
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ) as e:
        print(f"Failed to get video info: {e}")
        sys.exit(1)


def create_live_photo(
    video_path: str,
    output_dir: str = None,
    photo_format: str = "jpg",
    hq: bool = False,
    inject_metadata: bool = True,
):
    """
    Create Live Photo
    """
    video_path = Path(video_path).expanduser().resolve()
    if not video_path.exists():
        print(f"Error: File not found: {video_path}")
        sys.exit(1)

    photo_format = photo_format.lower()
    check_dependencies(
        needs_heif=(photo_format == "heic"), needs_makelive=inject_metadata
    )

    if output_dir is None:
        output_dir = video_path.parent
    else:
        output_dir = Path(output_dir).expanduser().resolve()
        output_dir.mkdir(parents=True, exist_ok=True)

    # Generate output filename
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    base_name = f"IMG_{timestamp}"

    img_ext = "HEIC" if photo_format == "heic" else "JPG"
    img_path = output_dir / f"{base_name}.{img_ext}"
    mov_path = output_dir / f"{base_name}.MOV"

    duration, resolution = get_video_info(video_path)
    video_duration = min(
        duration, 3.0
    )  # Live Photo videos are usually limited to 3 seconds

    print("\n[1/3] Preparing to create Live Photo")
    print(f"  Input Video: {video_path.name} ({resolution}, {duration:.2f}s)")
    print(f"  Output Format: {img_ext} + MOV")
    print(f"  Quality Mode: {'High Quality (HQ)' if hq else 'Standard'}")
    print(f"  Metadata Injection: {'Enabled' if inject_metadata else 'Disabled'}")

    # 1. Extract cover image
    print(f"\n[2/3] Extracting cover image ({img_ext})...")
    if photo_format == "heic":
        temp_png = output_dir / f"{base_name}_temp.png"
        subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-i",
                str(video_path),
                "-ss",
                "00:00:01",
                "-vframes",
                "1",
                "-pix_fmt",
                "rgb24",
                str(temp_png),
            ],
            check=True,
            capture_output=True,
        )

        heif_cmd = ["heif-enc"]
        if hq:
            heif_cmd.append("--lossless")
        else:
            heif_cmd.extend(["-q", "85"])
        heif_cmd.extend(["-o", str(img_path), str(temp_png)])
        subprocess.run(
            heif_cmd,
            check=True,
            capture_output=True,
        )
        temp_png.unlink(missing_ok=True)
    else:
        q_val = "1" if hq else "2"
        subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-i",
                str(video_path),
                "-ss",
                "00:00:01",
                "-vframes",
                "1",
                "-q:v",
                q_val,
                str(img_path),
            ],
            check=True,
            capture_output=True,
        )
    print(f"  ✓ Image generated: {img_path.name}")

    # 2. Convert video format
    print("\n[3/3] Converting video component...")
    ffmpeg_cmd = [
        "ffmpeg",
        "-y",
        "-i",
        str(video_path),
        "-t",
        str(video_duration),
        "-c:v",
        "h264",
        "-c:a",
        "aac",
        "-pix_fmt",
        "yuv420p",
        "-movflags",
        "+faststart",
    ]

    if hq:
        ffmpeg_cmd.extend(["-crf", "18", "-preset", "slow"])
    else:
        ffmpeg_cmd.extend(["-q:v", "2"])

    ffmpeg_cmd.append(str(mov_path))
    subprocess.run(ffmpeg_cmd, check=True, capture_output=True)
    print(f"  ✓ Video generated: {mov_path.name}")

    # 3. Inject Live Photo metadata (Optional)
    if inject_metadata:
        print("\nInjecting metadata using makelive...")
        try:
            subprocess.run(
                ["makelive", "-p", "-v", str(img_path), str(mov_path)],
                check=True,
                capture_output=True,
                text=True,
            )
            pvt_path = output_dir / f"{base_name}.pvt"
            print(f"  ✓ Live Photo package created: {pvt_path.name}")
        except (
            OSError,
            ValueError,
            RuntimeError,
            TypeError,
            KeyError,
            IndexError,
            AttributeError,
            UnicodeError,
        ) as e:
            print(f"  ! Metadata injection failed (makelive): {e}")
            print("  ! Keeping original image and video files.")

    print("\n" + "=" * 50)
    print(f"{pick_symbol('✨', ('[*]'))} Live Photo creation process complete!")
    print("=" * 50)
    print(f"File Location: {output_dir}")
    print(f" - {img_path.name}")
    print(f" - {mov_path.name}")
    if inject_metadata and (output_dir / f"{base_name}.pvt").exists():
        print(f" - {base_name}.pvt (Ready for import)")
    print("=" * 50 + "\n")


def main():
    parser = argparse.ArgumentParser(
        description="Convert video to Apple Live Photo format"
    )
    parser.add_argument("video", help="Input video file path")
    parser.add_argument("-o", "--output", help="Output directory (optional)")
    parser.add_argument(
        "-f",
        "--format",
        choices=["jpg", "heic"],
        default="jpg",
        help="Image format (default: jpg)",
    )
    parser.add_argument(
        "--hq", action="store_true", help="Enable high-quality encoding mode"
    )
    parser.add_argument(
        "--no-meta", action="store_true", help="Disable makelive metadata injection"
    )

    args = parser.parse_args()

    create_live_photo(
        video_path=args.video,
        output_dir=args.output,
        photo_format=args.format,
        hq=args.hq,
        inject_metadata=not args.no_meta,
    )


if __name__ == "__main__":
    main()

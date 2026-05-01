#!/usr/bin/env python3
import argparse
import os
import shutil
import subprocess
import sys
import time
from typing import Optional
from collections.abc import Callable

# collect_optimized.py v13
# Moves only optimized outputs into a mirrored directory tree.

GREEN = "\033[0;32m"
BLUE = "\033[0;34m"
YELLOW = "\033[1;33m"
CYAN = "\033[0;36m"
RED = "\033[0;31m"
NC = "\033[0m"

IMAGE_EXTENSIONS = {".jxl"}
VIDEO_EXTENSIONS = {".mov", ".mp4"}
TARGET_VIDEO_CODECS = {"hevc"}
PROBE_FAILURE_PREVIEW = 10

CodecProbe = Callable[[str], tuple[Optional[str], Optional[str]]]


def probe_video_codec(path: str) -> tuple[str | None, str | None]:
    """Returns the primary video codec name, or an error string."""
    cmd = [
        "ffprobe",
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=codec_name",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
        path,
    ]

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )
    except FileNotFoundError:
        return None, "ffprobe is not installed or not in PATH"
    except subprocess.TimeoutExpired:
        return None, "ffprobe timed out"
    except Exception as exc:  # pragma: no cover - defensive fallback
        return None, str(exc)

    if result.returncode != 0:
        error = (
            result.stderr.strip() or f"ffprobe exited with status {result.returncode}"
        )
        return None, error

    codec = result.stdout.strip().splitlines()
    if not codec:
        return None, "ffprobe returned no video codec"

    return codec[0].strip().lower(), None


def snapshot_directories(src_root: str):
    """Captures source directory times without following symlinked directories."""
    metadata = {}
    for root, dirnames, _ in os.walk(src_root):
        dirnames[:] = [
            name for name in dirnames if not os.path.islink(os.path.join(root, name))
        ]
        try:
            stat_result = os.stat(root)
        except OSError:
            continue
        metadata[root] = (stat_result.st_atime, stat_result.st_mtime)
    return metadata


def scan_candidates(src_root: str, codec_probe: CodecProbe):
    """Finds optimized files while ignoring symlinks."""
    candidates = []
    image_count = 0
    video_count = 0
    symlink_count = 0
    probe_failures = []

    for root, dirnames, filenames in os.walk(src_root):
        kept_dirs = []
        for dirname in dirnames:
            full_dir = os.path.join(root, dirname)
            if os.path.islink(full_dir):
                symlink_count += 1
                continue
            kept_dirs.append(dirname)
        dirnames[:] = kept_dirs

        for filename in filenames:
            full_path = os.path.join(root, filename)

            if os.path.islink(full_path):
                symlink_count += 1
                continue

            ext = os.path.splitext(filename)[1].lower()
            if ext in IMAGE_EXTENSIONS:
                candidates.append(full_path)
                image_count += 1
                continue

            if ext not in VIDEO_EXTENSIONS:
                continue

            codec, error = codec_probe(full_path)
            if error:
                probe_failures.append((full_path, error))
                continue

            if codec in TARGET_VIDEO_CODECS:
                candidates.append(full_path)
                video_count += 1

    return candidates, image_count, video_count, symlink_count, probe_failures


def ensure_destination_layout(
    src_root: str, dest_root: str, directory_metadata, dry_run: bool = False
) -> int:
    """Creates the full destination directory skeleton."""
    created_count = 0

    for src_dir in sorted(directory_metadata.keys(), key=len):
        rel_dir = os.path.relpath(src_dir, src_root)
        dest_dir = dest_root if rel_dir == "." else os.path.join(dest_root, rel_dir)

        if os.path.isdir(dest_dir):
            continue

        created_count += 1
        if not dry_run:
            os.makedirs(dest_dir, exist_ok=True)

    return created_count


def restore_directory_times(src_root: str, dest_root: str, directory_metadata) -> None:
    """Restores directory timestamps on both source and destination trees."""
    for src_dir in sorted(directory_metadata.keys(), key=len, reverse=True):
        times = directory_metadata[src_dir]
        rel_dir = os.path.relpath(src_dir, src_root)
        dest_dir = dest_root if rel_dir == "." else os.path.join(dest_root, rel_dir)

        if os.path.isdir(dest_dir):
            try:
                os.utime(dest_dir, times)
            except OSError:
                pass

        if os.path.isdir(src_dir):
            try:
                os.utime(src_dir, times)
            except OSError:
                pass


def prune_empty_source_directories(directory_metadata) -> int:
    """Removes source directories that are empty after relocation."""
    removed_count = 0

    for src_dir in sorted(directory_metadata.keys(), key=len, reverse=True):
        if not os.path.isdir(src_dir):
            continue

        try:
            if os.listdir(src_dir):
                continue
            os.rmdir(src_dir)
            removed_count += 1
        except OSError:
            continue

    return removed_count


def print_probe_failures(src_root: str, probe_failures) -> None:
    if not probe_failures:
        return

    print(f"{YELLOW}>>> Video probe failures: {len(probe_failures)}{NC}")
    for path, error in probe_failures[:PROBE_FAILURE_PREVIEW]:
        rel_path = os.path.relpath(path, src_root)
        print(f"{YELLOW}   - {rel_path}: {error}{NC}")

    remaining = len(probe_failures) - PROBE_FAILURE_PREVIEW
    if remaining > 0:
        print(f"{YELLOW}   ... and {remaining} more probe failures{NC}")


def validate_paths(src_root: str, dest_root: str) -> bool:
    if not os.path.isdir(src_root):
        print(f"{RED}Error: Source {src_root} is not a directory.{NC}")
        return False

    if os.path.exists(dest_root) and not os.path.isdir(dest_root):
        print(f"{RED}Error: Destination {dest_root} exists but is not a directory.{NC}")
        return False

    try:
        if os.path.commonpath([src_root, dest_root]) == src_root:
            print(
                f"{RED}Error: Destination directory cannot be inside the source directory.{NC}"
            )
            return False
    except ValueError:
        pass

    return True


def run_collection(
    source: str,
    destination: str,
    dry_run: bool = False,
    codec_probe: CodecProbe = probe_video_codec,
) -> bool:
    """Moves optimized files and mirrors the full source folder structure."""
    src_root = os.path.abspath(source)
    dest_root = os.path.abspath(destination)

    if not validate_paths(src_root, dest_root):
        return False

    # Mandatory confirmation using 'y'
    print(f"\n{BLUE}📂 COLLECTION TASK PREVIEW{NC}")
    print(f"   Source:      {CYAN}{src_root}{NC}")
    print(f"   Destination: {CYAN}{dest_root}{NC}")
    if dry_run:
        print(f"   {YELLOW}⚠️  DRY RUN MODE ENABLED{NC}")

    print(f"\n{YELLOW}⚠️  CONFIRM: Start collecting optimized media?{NC}")
    if input(
        f"   {BLUE}Type {GREEN}'yes'{BLUE} to proceed: {NC}"
    ).strip().lower() not in ("y", "yes"):
        print(f"\n{RED}❌ Task cancelled by user.{NC}")
        time.sleep(1)
        return False

    print(f"\n{BLUE}⏳ Initializing collection engine...{NC}")
    time.sleep(1.2)

    print(f"{BLUE}>>> Snapshotting directory structure and timestamps...{NC}")
    directory_metadata = snapshot_directories(src_root)

    print(f"{BLUE}>>> Scanning for optimized media in {src_root}...{NC}")
    candidates, image_count, video_count, symlink_count, probe_failures = (
        scan_candidates(src_root, codec_probe)
    )
    removed_empty_dirs = 0

    if not candidates:
        if not dry_run:
            removed_empty_dirs = prune_empty_source_directories(directory_metadata)
            if removed_empty_dirs > 0:
                print(
                    f"{BLUE}>>> Removed {removed_empty_dirs} empty source directory/directories.{NC}"
                )
        if symlink_count > 0:
            print(
                f"{YELLOW}No optimized files found ({symlink_count} symlinks ignored).{NC}"
            )
        else:
            print(f"{YELLOW}No optimized files found.{NC}")
        print_probe_failures(src_root, probe_failures)
        if removed_empty_dirs > 0:
            print(f"{BLUE}Removed Empty Source Directories: {removed_empty_dirs}{NC}")
        return True

    print(f"{BLUE}>>> Identified {len(candidates)} candidate files.{NC}")
    print(
        f"{BLUE}>>> Candidate breakdown: {image_count} JXL, {video_count} HEVC video(s).{NC}"
    )
    if symlink_count > 0:
        print(
            f"{YELLOW}>>> Note: {symlink_count} symlinks were ignored during the scan.{NC}"
        )
    print_probe_failures(src_root, probe_failures)

    mirrored_dirs = ensure_destination_layout(
        src_root, dest_root, directory_metadata, dry_run=dry_run
    )

    if dry_run:
        print(f"{YELLOW}--- DRY RUN MODE: No files will be moved ---{NC}")
        print(
            f"{YELLOW}[DRY-RUN] Would mirror {mirrored_dirs} directory/directories at destination.{NC}"
        )
    else:
        print(
            f"{BLUE}>>> Mirrored directory skeleton: {mirrored_dirs} directory/directories.{NC}"
        )

    moved_count = 0
    skipped_count = 0
    failed_moves = []

    for src_file in candidates:
        rel_path = os.path.relpath(src_file, src_root)
        dest_file = os.path.join(dest_root, rel_path)
        dest_dir = os.path.dirname(dest_file)

        if dry_run:
            print(f"{NC}[DRY-RUN] Would move: {rel_path}")
            continue

        try:
            os.makedirs(dest_dir, exist_ok=True)
            if os.path.exists(dest_file):
                print(f"{YELLOW}   Skipping (Exists at Target): {rel_path}{NC}")
                skipped_count += 1
                continue

            shutil.move(src_file, dest_file)
            print(f"{GREEN}   Moved: {rel_path}{NC}")
            moved_count += 1
        except Exception as exc:
            print(f"{RED}   FAILED: {rel_path} -> {exc}{NC}")
            failed_moves.append((rel_path, str(exc)))

    if not dry_run:
        removed_empty_dirs = prune_empty_source_directories(directory_metadata)
        if removed_empty_dirs > 0:
            print(
                f"{BLUE}>>> Removed {removed_empty_dirs} empty source directory/directories.{NC}"
            )
        print(f"{BLUE}>>> Restoring metadata for all mirrored directories...{NC}")
        restore_directory_times(src_root, dest_root, directory_metadata)

    status_color = GREEN if not failed_moves else RED
    print(f"\n{status_color}--- COLLECTION SUMMARY ---{NC}")
    print(f"Total Candidate Files: {len(candidates)}")
    print(f"Candidate JXL Images: {image_count}")
    print(f"Candidate HEVC Videos: {video_count}")
    print(f"Mirrored Directories: {mirrored_dirs}")
    print(f"Successfully Relocated: {moved_count}")
    print(f"Skipped (Target Exists): {skipped_count}")
    print(f"Skipped (Symlinks): {symlink_count}")
    print(f"Video Probe Failures: {len(probe_failures)}")
    print(f"Removed Empty Source Directories: {removed_empty_dirs}")

    if failed_moves:
        print(f"{RED}Failed Relocations: {len(failed_moves)}{NC}")
        for path, error in failed_moves:
            print(f"  - {path}: {error}")

    if dry_run:
        print(f"{YELLOW}Dry run complete. No changes were made.{NC}")
        return True

    print(
        f"{BLUE}Operation finished. Optimized files were moved, legacy files stayed in source when present, empty source directories were removed, and the directory tree was mirrored.{NC}"
    )
    return not failed_moves


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Collect optimized media files into a mirrored directory tree while "
            "leaving legacy formats in the original source directory."
        )
    )
    parser.add_argument("source", help="Source directory to scan")
    parser.add_argument("destination", help="Target directory to move files into")
    parser.add_argument(
        "--dry-run", action="store_true", help="Preview directory mirroring and moves"
    )
    args = parser.parse_args()

    if not run_collection(args.source, args.destination, args.dry_run):
        sys.exit(1)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print(f"\n{YELLOW}⚠️  Collection interrupted by user.{NC}")
        sys.exit(0)

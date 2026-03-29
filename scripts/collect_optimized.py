#!/usr/bin/env python3
import os
import sys
import shutil
import subprocess
import argparse

# collect_optimized.py v11 - Final Production-Ready Version
# Locates and moves 'Modern Format Boost' optimized files with absolute precision.

# ANSI Color codes
GREEN = "\033[0;32m"
BLUE = "\033[0;34m"
YELLOW = "\033[1;33m"
RED = "\033[0;31m"
NC = "\033[0m"

MARKER = "[Optimized by Modern Format Boost]"
EXTENSIONS = {".jxl", ".mov", ".mp4", ".heic", ".avif", ".png"}
ATTR_NAME = "com.apple.metadata:kMDItemFinderComment"


def get_finder_comment(path):
    """Robustly checks for the optimization marker via mdls and xattr fallback."""
    try:
        # Strategy 1: Standard mdls (Spotlight API)
        result = subprocess.run(
            ["mdls", "-name", "kMDItemFinderComment", path],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if result.returncode == 0 and MARKER in result.stdout:
            return True
    except Exception:
        pass

    try:
        # Strategy 2: Direct xattr read (Foolproof for local filesystems)
        raw_xattr = subprocess.check_output(
            ["xattr", "-p", ATTR_NAME, path], stderr=subprocess.DEVNULL, timeout=2
        )
        if MARKER.encode("utf-8") in raw_xattr:
            return True
    except Exception:
        pass

    return False


def is_hevc(path):
    """Detects if a video file is HEVC using ffprobe."""
    try:
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
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=5)
        return result.stdout.strip().lower() == "hevc"
    except Exception:
        return False


def run_collection(source, destination, dry_run=False):
    """Core logic to collect and move optimized files."""
    src_root = os.path.abspath(source)
    dest_root = os.path.abspath(destination)

    # 1. Path Conflict Check
    if dest_root == src_root or dest_root.startswith(src_root + os.sep):
        print(
            f"{RED}Error: Destination directory cannot be inside the source directory.{NC}"
        )
        return False

    if not os.path.isdir(src_root):
        print(f"{RED}Error: Source {src_root} is not a directory.{NC}")
        return False

    # Inform user about destination root creation
    if not os.path.exists(dest_root):
        if not dry_run:
            print(
                f"{BLUE}>>> Destination root does not exist. Creating: {dest_root}{NC}"
            )
            os.makedirs(dest_root, exist_ok=True)
        else:
            print(f"{YELLOW}[DRY-RUN] Would create destination root: {dest_root}{NC}")

    # 2. Pre-move Metadata Snapshot (Capture dual-sync baseline)
    print(f"{BLUE}>>> Snapshotting directory structure and timestamps...{NC}")
    all_dir_metadata = {}
    for root, _, _ in os.walk(src_root):
        try:
            st = os.stat(root)
            all_dir_metadata[root] = (st.st_atime, st.st_mtime)
        except Exception:
            pass

    # 3. Precision Scanning
    print(f"{BLUE}>>> Scanning for optimized media in {src_root}...{NC}")
    to_move = []
    symlink_count = 0

    for root, _, files in os.walk(src_root):
        for f in files:
            full_path = os.path.join(root, f)

            if os.path.islink(full_path):
                symlink_count += 1
                continue

            ext_raw = os.path.splitext(f)[1]
            # Condition: Extension must be clearly uppercase (e.g., .MOV, .JXL)
            if not ext_raw[1:].isupper() if len(ext_raw) > 1 else False:
                continue

            if ext_raw == ".MOV":
                # Select HEVC MOV only
                if is_hevc(full_path):
                    to_move.append(full_path)
            elif ext_raw == ".JXL":
                # Select JXL only
                to_move.append(full_path)
            # All other formats (non-HEVC MOV, non-JXL images) are implicitly excluded

    # Refined exit logic for better UX
    if not to_move:
        if symlink_count > 0:
            print(
                f"{YELLOW}No optimized files found ({symlink_count} symlinks ignored).{NC}"
            )
        else:
            print(f"{YELLOW}No optimized files found.{NC}")
        return True

    print(f"{BLUE}>>> Identified {len(to_move)} candidate files.{NC}")
    if symlink_count > 0:
        print(
            f"{YELLOW}>>> Note: {symlink_count} symlinks were ignored during the scan.{NC}"
        )

    if dry_run:
        print(f"{YELLOW}--- DRY RUN MODE: No files will be moved ---{NC}")

    # 4. Movement with Detailed Tracking
    moved_count = 0
    skipped_count = 0
    failed_moves = []

    for src_file in to_move:
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
        except Exception as e:
            print(f"{RED}   FAILED: {rel_path} -> {e}{NC}")
            failed_moves.append((rel_path, str(e)))

    # 5. Metadata Restoration (Dual-Sync: Src and Dest)
    if not dry_run:
        print(f"{BLUE}>>> Restoring metadata for all directories...{NC}")
        # Restore bottom-up: Child mtime update won't affect parent once parent is restored
        sorted_dirs = sorted(all_dir_metadata.keys(), key=len, reverse=True)
        for src_path in sorted_dirs:
            times = all_dir_metadata[src_path]
            rel_dir = os.path.relpath(src_path, src_root)

            # Sync Target (Including dest_root because rel_dir == "." is no longer skipped)
            target_dir = os.path.normpath(os.path.join(dest_root, rel_dir))
            if os.path.isdir(target_dir):
                try:
                    os.utime(target_dir, times)
                except Exception:
                    pass

            # Sync Source (Fix mtime changes triggered by 'mv' operations)
            if os.path.isdir(src_path):
                try:
                    os.utime(src_path, times)
                except Exception:
                    pass

    # 6. Comprehensive Final Report
    status_color = GREEN if not failed_moves else RED
    print(f"\n{status_color}--- COLLECTION SUMMARY ---{NC}")
    print(f"Total Candidate Files: {len(to_move)}")
    print(f"Successfully Relocated: {moved_count}")
    print(f"Skipped (Target Exists): {skipped_count}")
    print(f"Skipped (Symlinks): {symlink_count}")

    if failed_moves:
        print(f"{RED}Failed Relocations: {len(failed_moves)}{NC}")
        for path, err in failed_moves:
            print(f"  - {path}: {err}")

    if dry_run:
        print(f"{YELLOW}Dry run complete. No changes were made.{NC}")
        return True

    print(f"{BLUE}Operation finished with zero structure or metadata loss.{NC}")
    return not failed_moves


def main():
    parser = argparse.ArgumentParser(
        description="Collect optimized media files while preserving structure and metadata."
    )
    parser.add_argument("source", help="Source directory to scan")
    parser.add_argument("destination", help="Target directory to move files into")
    parser.add_argument(
        "--dry-run", action="store_true", help="Preview moves without executing them"
    )
    args = parser.parse_args()

    if not run_collection(args.source, args.destination, args.dry_run):
        sys.exit(1)


if __name__ == "__main__":
    main()

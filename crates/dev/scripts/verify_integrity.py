#!/usr/bin/env python3
"""Modern Format Boost - Post-Processing Integrity Verifier

Validates that every media file in the source directory has a corresponding
output in the optimized directory, ensuring zero data loss during batch
processing.

Usage:
    # Auto-detect mode: pass an __optimized directory, auto-finds the source
    python3 verify_integrity.py /path/to/MyPhotos_optimized

    # Explicit mode: pass source and optimized directories
    python3 verify_integrity.py /path/to/MyPhotos /path/to/MyPhotos_optimized
"""

import os
import sys
from pathlib import Path

# Console Output Colors
if sys.stdout.isatty():
    RED = "\033[38;5;196m"
    GREEN = "\033[38;5;46m"
    CYAN = "\033[38;5;51m"
    YELLOW = "\033[38;5;226m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    RESET = "\033[0m"
else:
    RED = GREEN = CYAN = YELLOW = BOLD = DIM = RESET = ""

# ---------------------------------------------------------------------------
# Media extension sets (mirrors drag_and_drop_processor.py)
# ---------------------------------------------------------------------------
IMG_EXTS = {
    ".jpg",
    ".jpeg",
    ".jpe",
    ".jfif",
    ".png",
    ".webp",
    ".heic",
    ".heif",
    ".avif",
    ".tiff",
    ".tif",
    ".bmp",
}

VID_EXTS = {
    ".gif",
    ".mp4",
    ".mov",
    ".mkv",
    ".avi",
    ".webm",
    ".m4v",
    ".wmv",
    ".flv",
}

# Modern Format Boost output formats (a source .png may become .jxl, etc.)
OUTPUT_EXTS = {
    ".jxl",
    ".avif",
    ".heic",
    ".heif",
    ".mp4",
    ".mov",
    ".mkv",
    ".webm",
    ".jpg",
    ".jpeg",
    ".png",
    ".webp",
    ".gif",
    ".tiff",
    ".tif",
    ".bmp",
}

MEDIA_EXTS = IMG_EXTS | VID_EXTS
ALL_KNOWN_EXTS = MEDIA_EXTS | OUTPUT_EXTS

# Extensions to always skip during verification
SKIP_EXTS = {".xmp", ".ds_store", ".thumbs.db", ".desktop.ini"}


def print_header():
    print()
    print(f"{CYAN}{BOLD}Modern Format Boost - Integrity Verifier{RESET}")
    print(f"{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}\n")


def is_media_file(path: Path) -> bool:
    """Check if file is a media file (excludes XMP, hidden files, etc.)."""
    if path.name.startswith("."):
        return False
    ext = path.suffix.lower()
    if ext in SKIP_EXTS:
        return False
    return ext in ALL_KNOWN_EXTS


def collect_media_files(directory: Path) -> dict[str, Path]:
    """Walk directory and collect all media files.

    Returns a dict mapping stem-based relative keys to full paths.
    The key is the relative path with the file extension stripped, enabling
    cross-format matching (e.g. source ``photo.png`` matches output ``photo.jxl``).
    """
    result: dict[str, Path] = {}
    for root, _, files in os.walk(directory):
        for fname in files:
            full = Path(root) / fname
            if not is_media_file(full):
                continue
            rel = full.relative_to(directory)
            # Key = relative path without extension (stem-based matching)
            key = str(rel.with_suffix("")).lower()
            result[key] = full
    return result


def resolve_directories(args: list[str]) -> tuple[Path, Path]:
    """Resolve source and optimized directories from CLI arguments.

    Supports:
    - 1 arg with ``__optimized`` suffix → auto-detect source
    - 1 arg without suffix → look for ``<name>_optimized`` sibling
    - 2 args → explicit source + optimized
    """
    if len(args) == 2:
        src = Path(args[0]).resolve()
        opt = Path(args[1]).resolve()
        return src, opt

    if len(args) == 1:
        given = Path(args[0]).resolve()

        # Case 1: given path ends with _optimized → derive source
        optimized_suffixes = ("_optimized", "__optimized")
        for suffix in optimized_suffixes:
            if given.name.endswith(suffix):
                source_name = given.name[: -len(suffix)]
                candidate = given.parent / source_name
                if candidate.is_dir():
                    return candidate, given
                # Try without the suffix variation
                break

        # Case 2: given path is the source → look for _optimized sibling
        for suffix in optimized_suffixes:
            candidate = given.parent / (given.name + suffix)
            if candidate.is_dir():
                return given, candidate

        print(f"{RED}❌ Could not auto-detect paired directory.{RESET}")
        print(f"   Given: {given}")
        print(
            f"   Expected sibling: {given.name}_optimized or {given.name}__optimized\n"
        )
        print(f"   {DIM}Tip: Pass both directories explicitly:{RESET}")
        print(
            f"   {DIM}  python3 verify_integrity.py /source/dir /optimized/dir{RESET}"
        )
        sys.exit(1)

    print(f"{RED}❌ Usage: verify_integrity.py <source_dir> [optimized_dir]{RESET}")
    sys.exit(1)


def format_size(size_bytes: int) -> str:
    """Format byte count into human-readable string."""
    if size_bytes < 1024:
        return f"{size_bytes} B"
    elif size_bytes < 1024 * 1024:
        return f"{size_bytes / 1024:.1f} KB"
    elif size_bytes < 1024 * 1024 * 1024:
        return f"{size_bytes / (1024 * 1024):.1f} MB"
    else:
        return f"{size_bytes / (1024 * 1024 * 1024):.2f} GB"


def verify_integrity(source_dir: Path, optimized_dir: Path) -> int:
    """Compare source and optimized directories for completeness.

    Returns exit code: 0 if all files accounted for, 1 if discrepancies found.
    """
    print(f"  {BOLD}Source:{RESET}    {source_dir}")
    print(f"  {BOLD}Optimized:{RESET} {optimized_dir}\n")

    if not source_dir.is_dir():
        print(f"{RED}❌ Source directory does not exist: {source_dir}{RESET}")
        return 1

    if not optimized_dir.is_dir():
        print(f"{RED}❌ Optimized directory does not exist: {optimized_dir}{RESET}")
        return 1

    # Collect files from both directories
    print(f"{DIM}   Scanning source directory...{RESET}", end="", flush=True)
    source_files = collect_media_files(source_dir)
    print(f" {GREEN}{len(source_files)} media files{RESET}")

    print(f"{DIM}   Scanning optimized directory...{RESET}", end="", flush=True)
    optimized_files = collect_media_files(optimized_dir)
    print(f" {GREEN}{len(optimized_files)} media files{RESET}\n")

    if not source_files:
        print(f"{YELLOW}⚠️  No media files found in source directory.{RESET}")
        return 0

    # Cross-reference: every source file should have a match in optimized
    missing: list[tuple[str, Path]] = []
    matched: list[tuple[str, Path, Path]] = []
    extra: list[tuple[str, Path]] = []

    for key, src_path in sorted(source_files.items()):
        if key in optimized_files:
            matched.append((key, src_path, optimized_files[key]))
        else:
            missing.append((key, src_path))

    for key, opt_path in sorted(optimized_files.items()):
        if key not in source_files:
            extra.append((key, opt_path))

    # Calculate size statistics for matched files
    total_src_size = 0
    total_opt_size = 0
    for _, src_path, opt_path in matched:
        try:
            total_src_size += src_path.stat().st_size
        except OSError:
            pass
        try:
            total_opt_size += opt_path.stat().st_size
        except OSError:
            pass

    # Print results
    print(f"{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}")
    print(f"{BOLD}Verification Results:{RESET}\n")

    # Matched files summary
    match_rate = (len(matched) / len(source_files) * 100) if source_files else 0
    if match_rate == 100:
        print(
            f"  {GREEN}✅ Match Rate: {match_rate:.1f}% ({len(matched)}/{len(source_files)}){RESET}"
        )
    elif match_rate >= 90:
        print(
            f"  {YELLOW}⚠️  Match Rate: {match_rate:.1f}% ({len(matched)}/{len(source_files)}){RESET}"
        )
    else:
        print(
            f"  {RED}❌ Match Rate: {match_rate:.1f}% ({len(matched)}/{len(source_files)}){RESET}"
        )

    # Size comparison
    if total_src_size > 0:
        savings = total_src_size - total_opt_size
        savings_pct = (savings / total_src_size * 100) if total_src_size > 0 else 0
        print(f"  {DIM}   Source total:    {format_size(total_src_size)}{RESET}")
        print(f"  {DIM}   Optimized total: {format_size(total_opt_size)}{RESET}")
        if savings > 0:
            print(
                f"  {GREEN}   Space saved:     {format_size(savings)} ({savings_pct:.1f}%){RESET}"
            )
        else:
            print(
                f"  {YELLOW}   Size increase:   {format_size(-savings)} (+{-savings_pct:.1f}%){RESET}"
            )

    # Missing files detail
    if missing:
        print(f"\n  {RED}❌ Missing from optimized ({len(missing)} files):{RESET}")
        # Group by directory for readability
        shown = 0
        max_show = 50
        for key, src_path in missing:
            if shown >= max_show:
                remaining = len(missing) - max_show
                print(f"     {DIM}... and {remaining} more{RESET}")
                break
            rel = src_path.relative_to(source_dir)
            print(f"     {RED}✗{RESET} {rel}")
            shown += 1

    # Extra files in optimized (informational)
    if extra:
        print(f"\n  {CYAN}ℹ️  Extra files in optimized ({len(extra)} files):{RESET}")
        shown = 0
        max_show = 20
        for key, opt_path in extra:
            if shown >= max_show:
                remaining = len(extra) - max_show
                print(f"     {DIM}... and {remaining} more{RESET}")
                break
            rel = opt_path.relative_to(optimized_dir)
            print(f"     {CYAN}+{RESET} {rel}")
            shown += 1

    # Subdirectory structure comparison
    src_dirs = set()
    for root, dirs, _ in os.walk(source_dir):
        for d in dirs:
            rel = Path(root, d).relative_to(source_dir)
            src_dirs.add(str(rel).lower())

    opt_dirs = set()
    for root, dirs, _ in os.walk(optimized_dir):
        for d in dirs:
            rel = Path(root, d).relative_to(optimized_dir)
            opt_dirs.add(str(rel).lower())

    missing_dirs = src_dirs - opt_dirs
    if missing_dirs:
        print(f"\n  {YELLOW}⚠️  Missing subdirectories ({len(missing_dirs)}):{RESET}")
        for d in sorted(missing_dirs)[:20]:
            print(f"     {YELLOW}✗{RESET} {d}/")

    # Final verdict
    print(f"\n{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}")
    if not missing and not missing_dirs:
        print(
            f"{GREEN}{BOLD}🌟 INTEGRITY VERIFIED: All {len(source_files)} media files accounted for.{RESET}"
        )
        return 0
    else:
        total_issues = len(missing) + len(missing_dirs)
        print(
            f"{RED}{BOLD}⚠️  INTEGRITY CHECK FAILED: {total_issues} issue(s) detected.{RESET}"
        )
        print(f"{DIM}   Processing may be incomplete. Check logs for errors.{RESET}")
        return 1


def main():
    print_header()
    source_dir, optimized_dir = resolve_directories(sys.argv[1:])
    exit_code = verify_integrity(source_dir, optimized_dir)
    print()
    sys.exit(exit_code)


if __name__ == "__main__":
    main()

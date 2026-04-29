#!/usr/bin/env python3
"""Modern Format Boost - Post-Processing Integrity Verifier

Checks that every media file in the source directory has a corresponding
output in the optimized directory. Reports any discrepancies with full
detail and writes a log file to the project logs/ directory.

Usage:
    # Auto-detect mode: pass an __optimized directory, auto-finds the source
    python3 verify_integrity.py /path/to/MyPhotos_optimized

    # Explicit mode: pass source and optimized directories
    python3 verify_integrity.py /path/to/MyPhotos /path/to/MyPhotos_optimized
"""

import datetime
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

SCRIPT_DIR = Path(__file__).parent.resolve()
PROJECT_ROOT = SCRIPT_DIR.parent.parent.parent
LOG_DIR = PROJECT_ROOT / "logs"

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
        sys.exit(0)

    print(f"{RED}❌ Usage: verify_integrity.py <source_dir> [optimized_dir]{RESET}")
    sys.exit(0)


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


def write_log(
    source_dir: Path,
    optimized_dir: Path,
    source_files: dict[str, Path],
    optimized_files: dict[str, Path],
    matched: list[tuple[str, Path, Path]],
    missing: list[tuple[str, Path]],
    extra: list[tuple[str, Path]],
    missing_dirs: set[str],
) -> Path:
    """Write detailed verification report to a log file in the logs/ directory."""
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
    project_name = source_dir.name
    log_path = LOG_DIR / f"verify_{project_name}_{timestamp}.log"

    with open(log_path, "w", encoding="utf-8") as f:
        f.write("=" * 72 + "\n")
        f.write("Modern Format Boost - Integrity Verification Report\n")
        f.write(f"Generated: {datetime.datetime.now().isoformat()}\n")
        f.write("=" * 72 + "\n\n")

        f.write(f"Source:    {source_dir}\n")
        f.write(f"Optimized: {optimized_dir}\n\n")

        # Count summary
        f.write("--- File Count Summary ---\n")
        f.write(f"Source files:    {len(source_files)}\n")
        f.write(f"Optimized files: {len(optimized_files)}\n")
        delta = len(optimized_files) - len(source_files)
        if delta == 0:
            f.write("Count status:    MATCH\n")
        else:
            direction = "more" if delta > 0 else "fewer"
            f.write(
                f"Count status:    MISMATCH ({abs(delta)} {direction} in optimized)\n"
            )

        f.write(f"\nMatched:   {len(matched)}\n")
        f.write(f"Missing:   {len(missing)}\n")
        f.write(f"Extra:     {len(extra)}\n\n")

        # Matched files
        f.write("--- Matched Files ---\n")
        for key, src_path, opt_path in matched:
            src_rel = src_path.relative_to(source_dir)
            opt_rel = opt_path.relative_to(optimized_dir)
            try:
                src_sz = src_path.stat().st_size
                opt_sz = opt_path.stat().st_size
                f.write(
                    f"  ✓ {src_rel} → {opt_rel}  ({format_size(src_sz)} → {format_size(opt_sz)})\n"
                )
            except OSError:
                f.write(f"  ✓ {src_rel} → {opt_rel}\n")

        # Missing files
        if missing:
            f.write(f"\n--- Missing from Optimized ({len(missing)}) ---\n")
            for key, src_path in missing:
                rel = src_path.relative_to(source_dir)
                try:
                    sz = src_path.stat().st_size
                    f.write(f"  ✗ {rel}  ({format_size(sz)})\n")
                except OSError:
                    f.write(f"  ✗ {rel}\n")

        # Extra files
        if extra:
            f.write(f"\n--- Extra in Optimized ({len(extra)}) ---\n")
            for key, opt_path in extra:
                rel = opt_path.relative_to(optimized_dir)
                try:
                    sz = opt_path.stat().st_size
                    f.write(f"  + {rel}  ({format_size(sz)})\n")
                except OSError:
                    f.write(f"  + {rel}\n")

        # Missing directories
        if missing_dirs:
            f.write(f"\n--- Missing Subdirectories ({len(missing_dirs)}) ---\n")
            for d in sorted(missing_dirs):
                f.write(f"  ✗ {d}/\n")

        f.write("\n" + "=" * 72 + "\n")
        f.write("End of Report\n")

    return log_path


def verify_integrity(source_dir: Path, optimized_dir: Path):
    """Compare source and optimized directories and report findings."""
    print(f"  {BOLD}Source:{RESET}    {source_dir}")
    print(f"  {BOLD}Optimized:{RESET} {optimized_dir}\n")

    if not source_dir.is_dir():
        print(f"{RED}❌ Source directory does not exist: {source_dir}{RESET}")
        return

    if not optimized_dir.is_dir():
        print(f"{RED}❌ Optimized directory does not exist: {optimized_dir}{RESET}")
        return

    # Collect files from both directories
    print(f"{DIM}   Scanning source directory...{RESET}", end="", flush=True)
    source_files = collect_media_files(source_dir)
    print(f" {GREEN}{len(source_files)} media files{RESET}")

    print(f"{DIM}   Scanning optimized directory...{RESET}", end="", flush=True)
    optimized_files = collect_media_files(optimized_dir)
    print(f" {GREEN}{len(optimized_files)} media files{RESET}\n")

    if not source_files:
        print(f"{YELLOW}⚠️  No media files found in source directory.{RESET}")
        return

    # =========================================================================
    # File Count Consistency
    # =========================================================================
    src_count = len(source_files)
    opt_count = len(optimized_files)

    # Break down by category
    src_img = sum(1 for p in source_files.values() if p.suffix.lower() in IMG_EXTS)
    src_vid = sum(1 for p in source_files.values() if p.suffix.lower() in VID_EXTS)
    opt_img = sum(
        1 for p in optimized_files.values() if p.suffix.lower() in (IMG_EXTS | {".jxl"})
    )
    opt_vid = sum(
        1
        for p in optimized_files.values()
        if p.suffix.lower() in VID_EXTS
        and p.suffix.lower() not in (IMG_EXTS | {".jxl"})
    )

    print(f"{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}")
    print(f"{BOLD}File Count Consistency{RESET}\n")

    if src_count == opt_count:
        print(f"  {GREEN}✅ Total Count: {src_count} == {opt_count} (MATCH){RESET}")
    else:
        delta = opt_count - src_count
        direction = "more" if delta > 0 else "fewer"
        print(
            f"  {YELLOW}⚠️  Total Count: Source={src_count}  Optimized={opt_count}  "
            f"({abs(delta)} {direction}){RESET}"
        )

    print(f"  {DIM}   Source  → Images: {src_img}  Videos: {src_vid}{RESET}")
    print(f"  {DIM}   Output  → Images: {opt_img}  Videos: {opt_vid}{RESET}")

    # =========================================================================
    # Stem-Based Cross-Reference
    # =========================================================================
    print(f"\n{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}")
    print(f"{BOLD}Stem-Based Cross-Reference{RESET}\n")

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

    # Match rate
    match_rate = (len(matched) / len(source_files) * 100) if source_files else 0
    if match_rate == 100:
        print(
            f"  {GREEN}✅ Match Rate: {match_rate:.1f}% "
            f"({len(matched)}/{len(source_files)}){RESET}"
        )
    elif match_rate >= 90:
        print(
            f"  {YELLOW}⚠️  Match Rate: {match_rate:.1f}% "
            f"({len(matched)}/{len(source_files)}){RESET}"
        )
    else:
        print(
            f"  {RED}   Match Rate: {match_rate:.1f}% "
            f"({len(matched)}/{len(source_files)}){RESET}"
        )

    # Size comparison for matched files
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

    if total_src_size > 0:
        savings = total_src_size - total_opt_size
        savings_pct = (savings / total_src_size * 100) if total_src_size > 0 else 0
        print(f"  {DIM}   Source total:    {format_size(total_src_size)}{RESET}")
        print(f"  {DIM}   Optimized total: {format_size(total_opt_size)}{RESET}")
        if savings > 0:
            print(
                f"  {GREEN}   Space saved:     "
                f"{format_size(savings)} ({savings_pct:.1f}%){RESET}"
            )
        else:
            print(
                f"  {YELLOW}   Size increase:   "
                f"{format_size(-savings)} (+{-savings_pct:.1f}%){RESET}"
            )

    # =========================================================================
    # Discrepancy Details
    # =========================================================================
    if missing or extra:
        print(f"\n{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}")
        print(f"{BOLD}Discrepancy Details{RESET}\n")

    if missing:
        print(f"  {YELLOW}Missing from optimized ({len(missing)} files):{RESET}")
        shown = 0
        max_show = 30
        for key, src_path in missing:
            if shown >= max_show:
                remaining = len(missing) - max_show
                print(f"     {DIM}... and {remaining} more (see log file){RESET}")
                break
            rel = src_path.relative_to(source_dir)
            print(f"     {YELLOW}✗{RESET} {rel}")
            shown += 1

    if extra:
        print(f"\n  {CYAN}Extra in optimized ({len(extra)} files):{RESET}")
        shown = 0
        max_show = 20
        for key, opt_path in extra:
            if shown >= max_show:
                remaining = len(extra) - max_show
                print(f"     {DIM}... and {remaining} more (see log file){RESET}")
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
        print(f"\n  {YELLOW}Missing subdirectories ({len(missing_dirs)}):{RESET}")
        for d in sorted(missing_dirs)[:20]:
            print(f"     {YELLOW}✗{RESET} {d}/")

    # =========================================================================
    # Write log file
    # =========================================================================
    log_path = write_log(
        source_dir,
        optimized_dir,
        source_files,
        optimized_files,
        matched,
        missing,
        extra,
        missing_dirs,
    )

    # =========================================================================
    # Summary
    # =========================================================================
    print(f"\n{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}")
    has_issues = missing or (src_count != opt_count) or missing_dirs

    if not has_issues:
        print(
            f"{GREEN}{BOLD}🌟 All {len(source_files)} media files accounted for "
            f"({src_count} → {opt_count}).{RESET}"
        )
    else:
        parts: list[str] = []
        if src_count != opt_count:
            parts.append(f"count: {src_count} vs {opt_count}")
        if missing:
            parts.append(f"{len(missing)} missing")
        if missing_dirs:
            parts.append(f"{len(missing_dirs)} dirs missing")
        print(f"{YELLOW}{BOLD}⚠️  Discrepancies found: {', '.join(parts)}{RESET}")

    print(f"  {DIM}📄 Full report saved to: {log_path}{RESET}")


def main():
    print_header()
    source_dir, optimized_dir = resolve_directories(sys.argv[1:])
    verify_integrity(source_dir, optimized_dir)
    print()


if __name__ == "__main__":
    main()

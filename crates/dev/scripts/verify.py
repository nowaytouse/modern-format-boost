#!/usr/bin/env python3
"""Modern Format Boost - Conversion Analyzer & Integrity Verifier

Combines log-based conversion analysis with filesystem-level integrity checking.
Extracts modern format conversions, loop intent edge cases (Uncertain/KNN Bypass),
and verifies that every source file has a corresponding optimized output.

Usage:
    # 1. Log analysis (auto-scans directory for .log and error files)
    python3 verify.py logs/

    # 2. Integrity verification (auto-detects paired source/optimized folder)
    python3 verify.py --verify /path/to/MyPhotos_optimized

    # 3. Integrity verification (explicitly specify both directories)
    python3 verify.py --verify /path/to/Source /path/to/Optimized

    # 4. Combined analysis (log scanning + auto-detecting verification)
    python3 verify.py logs/ --verify /path/to/MyPhotos

    # 5. Combined analysis (log scanning + explicit dual-directory verification)
    python3 verify.py logs/ --verify /path/to/Source /path/to/Optimized
"""
import argparse
import hashlib
import os
import re
import sys
from datetime import datetime
from pathlib import Path


# Console Output Colors
if sys.stdout.isatty():
    RED = "\033[38;5;196m"
    GREEN = "\033[38;5;46m"
    CYAN = "\033[38;5;51m"
    YELLOW = "\033[38;5;226m"
    BLUE = "\033[0;34m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    NC = "\033[0m"
    RESET = "\033[0m"
else:
    RED = GREEN = CYAN = YELLOW = BLUE = BOLD = DIM = NC = RESET = ""

# ---------------------------------------------------------------------------
# Constants from Integrity Verifier
# ---------------------------------------------------------------------------
IMG_EXTS = {
    ".jpg", ".jpeg", ".jpe", ".jfif", ".png", ".webp",
    ".heic", ".heif", ".avif", ".tiff", ".tif", ".bmp",
    ".ico", ".svg", ".jp2", ".j2k", ".jxl",
}

VID_EXTS = {
    ".gif", ".mp4", ".mov", ".mkv", ".avi", ".webm",
    ".m4v", ".wmv", ".flv", ".mpg", ".mpeg", ".ts",
    ".mts", ".m2ts", ".m2v", ".3gp", ".3g2", ".ogv",
    ".f4v", ".asf", ".apng",
}

OUTPUT_EXTS = {
    ".jxl", ".avif", ".heic", ".heif", ".mp4", ".mov",
    ".mkv", ".webm", ".jpg", ".jpeg", ".png", ".webp",
    ".gif", ".tiff", ".tif", ".bmp", ".apng",
}

MEDIA_EXTS = IMG_EXTS | VID_EXTS
ALL_KNOWN_EXTS = MEDIA_EXTS | OUTPUT_EXTS
SKIP_EXTS = {".xmp", ".ds_store", ".thumbs.db", ".desktop.ini"}

# ---------------------------------------------------------------------------
# Integrity Verification Logic
# ---------------------------------------------------------------------------

def is_media_file(path: Path) -> bool:
    """Check if file is a media file (excludes XMP, hidden files, etc.)."""
    if path.name.startswith("."):
        return False
    ext = path.suffix.lower()
    if ext in SKIP_EXTS:
        return False
    return ext in ALL_KNOWN_EXTS


def file_content_hash(path: Path, chunk_size: int = 65536) -> str:
    """Compute SHA-256 of first 64KB for fast identity comparison."""
    h = hashlib.sha256()
    try:
        with open(path, "rb") as f:
            h.update(f.read(chunk_size))
    except OSError:
        return "ERROR"
    return h.hexdigest()[:16]


def collect_media_files(directory: Path) -> dict[str, list[Path]]:
    """Walk directory and collect all media files, tracking potential collisions."""
    result: dict[str, list[Path]] = {}
    for root, _, files in os.walk(directory):
        for fname in files:
            full = Path(root) / fname
            if not is_media_file(full):
                continue
            try:
                rel = full.relative_to(directory)
                # The 'key' is the relative path without suffix, lowercase.
                # This helps match 'Folder/Img.jpg' with 'folder/img.jxl'.
                key = str(rel.with_suffix("")).lower()
                if key not in result:
                    result[key] = []
                result[key].append(full)
            except ValueError:
                continue
    return result


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


def resolve_verify_dirs(args_verify: list[str]) -> tuple[Path, Path] | None:
    """Resolve source and optimized directories with auto-detection."""
    if len(args_verify) >= 2:
        return Path(args_verify[0]).resolve(), Path(args_verify[1]).resolve()

    given = Path(args_verify[0]).resolve()
    optimized_suffixes = ("_optimized", "__optimized")

    # Case 1: given path ends with _optimized → derive source
    for suffix in optimized_suffixes:
        if given.name.endswith(suffix):
            source_name = given.name[:-len(suffix)]
            candidate = given.parent / source_name
            if candidate.is_dir():
                return candidate, given

    # Case 2: given path is the source → look for _optimized sibling
    for suffix in optimized_suffixes:
        candidate = given.parent / (given.name + suffix)
        if candidate.is_dir():
            return given, candidate

    return None


def run_integrity_check(source_dir: Path, optimized_dir: Path, report_f):
    """Perform integrity check and write results to the open report file handle."""
    report_f.write("── INTEGRITY VERIFICATION ─────────────────────────────────────\n")
    report_f.write(f"Source:    {source_dir}\n")
    report_f.write(f"Optimized: {optimized_dir}\n\n")

    if not source_dir.is_dir() or not optimized_dir.is_dir():
        report_f.write("❌ Error: Source or Optimized directory missing.\n\n")
        return

    source_files = collect_media_files(source_dir)
    optimized_files = collect_media_files(optimized_dir)

    # Collision Detection (Security hardening)
    src_collisions = {k: v for k, v in source_files.items() if len(v) > 1}
    opt_collisions = {k: v for k, v in optimized_files.items() if len(v) > 1}

    src_count = sum(len(v) for v in source_files.values())
    opt_count = sum(len(v) for v in optimized_files.values())

    report_f.write(f"Source files:    {src_count}\n")
    report_f.write(f"Optimized files: {opt_count}\n")

    delta = opt_count - src_count
    if delta == 0:
        report_f.write("Count status:    MATCH\n")
    else:
        direction = "more" if delta > 0 else "fewer"
        report_f.write(f"Count status:    MISMATCH ({abs(delta)} {direction} in optimized)\n")

    # Matching logic
    missing = []
    matched = []  # List of (key, src_path, opt_path)
    extra = []
    ambiguous = [] # 1-to-N or N-to-1 matches

    # 1. Identify matches and missing
    for key, src_paths in sorted(source_files.items()):
        if key in optimized_files:
            opt_paths = optimized_files[key]
            # If it's a clean 1-to-1 match
            if len(src_paths) == 1 and len(opt_paths) == 1:
                matched.append((key, src_paths[0], opt_paths[0]))
            else:
                # N-to-M collision or ambiguity
                ambiguous.append((key, src_paths, opt_paths))
        else:
            for p in src_paths:
                missing.append((key, p))

    # 2. Identify extras
    for key, opt_paths in sorted(optimized_files.items()):
        if key not in source_files:
            for p in opt_paths:
                extra.append((key, p))

    report_f.write(f"Matched:         {len(matched)}\n")
    report_f.write(f"Ambiguous:       {len(ambiguous)} (Collisions detected!)\n")
    report_f.write(f"Missing:         {len(missing)}\n")
    report_f.write(f"Extra:           {len(extra)}\n\n")

    if src_collisions or opt_collisions or ambiguous:
        report_f.write("── COLLISIONS & SAFETY WARNINGS ───────────────────────────────\n")
        if src_collisions:
            report_f.write("⚠️ WARNING: Duplicate source stems detected (Unsafe for 1-to-1 mapping):\n")
            for key, paths in sorted(src_collisions.items()):
                hashes = [file_content_hash(p) for p in paths]
                unique_h = len(set(hashes))
                label = "IDENTICAL content" if unique_h == 1 else f"{unique_h} DISTINCT files"
                report_f.write(f"  Key '{key}' maps to {len(paths)} files ({label}):\n")
                for p, h in zip(paths, hashes):
                    report_f.write(f"    - {p.relative_to(source_dir)}  [sha256:{h}]\n")
            report_f.write("\n")

        if opt_collisions:
            report_f.write("⚠️ WARNING: Duplicate optimized stems detected (Potential overwrites):\n")
            for key, paths in sorted(opt_collisions.items()):
                hashes = [file_content_hash(p) for p in paths]
                unique_h = len(set(hashes))
                label = "IDENTICAL content" if unique_h == 1 else f"{unique_h} DISTINCT files"
                report_f.write(f"  Key '{key}' maps to {len(paths)} outputs ({label}):\n")
                for p, h in zip(paths, hashes):
                    report_f.write(f"    - {p.relative_to(optimized_dir)}  [sha256:{h}]\n")
            report_f.write("\n")

        if ambiguous:
            report_f.write("⚠️ WARNING: Ambiguous mapping (N-to-M relationship):\n")
            for key, srcs, opts in sorted(ambiguous):
                report_f.write(f"  Key '{key}':\n")
                report_f.write("    Sources:\n")
                for p in srcs:
                    report_f.write(f"      - {p.name}\n")
                report_f.write("    Optimized:\n")
                for p in opts:
                    report_f.write(f"      - {p.name}\n")
            report_f.write("\n")

    # 3. Type Mismatch Check (Content Consistency)
    mismatched_types = []
    for key, src_p, opt_p in matched:
        src_is_vid = src_p.suffix.lower() in VID_EXTS
        opt_is_vid = opt_p.suffix.lower() in VID_EXTS
        if src_is_vid != opt_is_vid:
            mismatched_types.append((src_p, opt_p))

    if mismatched_types:
        report_f.write("── CONTENT CONSISTENCY WARNINGS ───────────────────────────────\n")
        report_f.write("⚠️ WARNING: Media type mismatch detected (Suspicious conversion):\n")
        for src, opt in mismatched_types:
            s_type = "Video" if src.suffix.lower() in VID_EXTS else "Image"
            o_type = "Video" if opt.suffix.lower() in VID_EXTS else "Image"
            report_f.write(f"  - {src.name} ({s_type}) → {opt.name} ({o_type})\n")
        report_f.write("\n")

    if missing:
        report_f.write(f"--- Missing from Optimized ({len(missing)}) ---\n")
        for _, src_path in missing:
            rel = src_path.relative_to(source_dir)
            report_f.write(f"  ✗ {rel}\n")
        report_f.write("\n")

    if extra:
        report_f.write(f"--- Extra in Optimized ({len(extra)}) ---\n")
        for _, opt_path in extra:
            rel = opt_path.relative_to(optimized_dir)
            report_f.write(f"  + {rel}\n")
        report_f.write("\n")

    # Space savings
    total_src_size = 0
    total_opt_size = 0
    for _, src_path, opt_path in matched:
        try:
            total_src_size += src_path.stat().st_size
            total_opt_size += opt_path.stat().st_size
        except OSError:
            pass

    if total_src_size > 0:
        savings = total_src_size - total_opt_size
        savings_pct = (savings / total_src_size * 100)
        report_f.write("--- Storage Impact (Matched Files) ---\n")
        report_f.write(f"  Source total:    {format_size(total_src_size)}\n")
        report_f.write(f"  Optimized total: {format_size(total_opt_size)}\n")
        report_f.write(f"  Space saved:     {format_size(savings)} ({savings_pct:.1f}%)\n\n")

# ---------------------------------------------------------------------------
# Log Analysis Logic
# ---------------------------------------------------------------------------

def parse_logs(log_paths, report_f, filter_dir=None):
    """Analyze logs and write results to the report file handle.
    
    If filter_dir is provided, only entries belonging to that directory tree
    will be included in the report.
    """
    result_pattern = re.compile(r"([\S\s]+?)\s*→\s*([\S\s]+?)\s*\(([^)]+)\)\s*([✅❌])")
    activity_pattern = re.compile(r"🔄\s*Animated→([A-Z0-9\s]+)\s*\(([^)]+)\):\s*(.+)")
    checking_pattern = re.compile(r"checking\s+([^\s]+)$")
    uncertain_pattern = re.compile(r"Tree uncertain \(([^)]+)\) \[prob=([\d.]+)\].*falling back to Layer 6 KNN")
    knn_bypass_pattern = re.compile(r"Loop DB unavailable or disabled — running tree without KNN")
    tree_uncertain_pattern = re.compile(r"Tree-only result remained uncertain \(([^)]+)\)")

    modern_exts = {".webp", ".avif", ".jxl", ".heic", ".heif"}
    target_formats = {"GIF", "MOV", "MP4", "HEVC", "AV1"}

    results = []
    uncertain_cases = []
    log_dir_path = Path("logs")
    
    # Pre-resolve filter_dir if provided
    filter_dir_abs = str(Path(filter_dir).resolve()) if filter_dir else None

    for path in log_paths:
        p = Path(path)
        if p.is_dir():
            files = list(p.glob("**/*.log")) + list(p.glob("**/error"))
        else:
            files = [p]

        for log_file in files:
            current_file = None
            try:
                with open(log_file, encoding="utf-8", errors="ignore") as f:
                    for line in f:
                        line = line.strip()
                        if not line:
                            continue

                        # Track file
                        if (m := checking_pattern.search(line)):
                            current_file = m.group(1).strip()

                        # Check if current_file is within filter_dir
                        is_relevant = True
                        if filter_dir_abs and current_file:
                            try:
                                abs_current = str(Path(current_file).resolve())
                                if not abs_current.startswith(filter_dir_abs):
                                    is_relevant = False
                            except Exception:
                                pass

                        if not is_relevant:
                            continue

                        # Conversions
                        if (m := result_pattern.search(line)):
                            source = m.group(1).split(">")[-1].strip()
                            current_file = source
                            
                            # Re-verify relevance for conversion line
                            if filter_dir_abs:
                                try:
                                    if not str(Path(source).resolve()).startswith(filter_dir_abs):
                                        continue
                                except Exception:
                                    pass

                            target, msg, status_icon = m.group(2).strip(), m.group(3).strip(), m.group(4)
                            if Path(source).suffix.lower() in modern_exts and any(f in target.upper() or f in msg.upper() for f in target_formats):
                                results.append({"log": log_file.name, "source": source, "target": target, "status": "SUCCESS" if status_icon == "✅" else "FAILED", "details": msg})

                        if (m := activity_pattern.search(line)):
                            target_fmt, details, source = m.group(1).strip(), m.group(2).strip(), m.group(3).strip()
                            current_file = source
                            
                            # Re-verify relevance
                            if filter_dir_abs:
                                try:
                                    if not str(Path(source).resolve()).startswith(filter_dir_abs):
                                        continue
                                except Exception:
                                    pass

                            if Path(source).suffix.lower() in modern_exts and any(f in target_fmt.upper() for f in target_formats):
                                results.append({"log": log_file.name, "source": source, "target": f"CONVERTED TO {target_fmt}", "status": "PROCESSING/UNKNOWN", "details": details})

                        # Loop Intent
                        if uncertain_pattern.search(line) or knn_bypass_pattern.search(line) or tree_uncertain_pattern.search(line):
                            if current_file:
                                reason, prob = "N/A", "N/A"
                                if (u := uncertain_pattern.search(line)):
                                    reason, prob = u.group(1), u.group(2)
                                elif knn_bypass_pattern.search(line):
                                    reason = "KNN Bypassed (DB Unavailable)"
                                elif (t := tree_uncertain_pattern.search(line)):
                                    reason = t.group(1)

                                # Duplicate check
                                if not any(c["file"] == current_file and c["log"] == log_file.name for c in uncertain_cases):
                                    matching_folders = []
                                    if log_dir_path.exists():
                                        stem = Path(current_file).stem
                                        for item in log_dir_path.iterdir():
                                            if item.is_dir() and stem in item.name:
                                                matching_folders.append(item.name)
                                    
                                    uncertain_cases.append({"file": current_file, "reason": reason, "probability": prob, "log": log_file.name, "matching_folders": matching_folders})

            except Exception as e:
                print(f"⚠️ Error reading {log_file}: {e}", file=sys.stderr)

    # Dedup and write
    unique_results = []
    seen_res = set()
    for r in results:
        if (r["source"], r["target"]) not in seen_res:
            unique_results.append(r)
            seen_res.add((r["source"], r["target"]))

    unique_uncertain = []
    seen_unc = set()
    for c in uncertain_cases:
        if c["file"] not in seen_unc:
            unique_uncertain.append(c)
            seen_unc.add(c["file"])

    report_f.write("── LOOP INTENT EDGE CASES (UNCERTAIN / KNN BYPASSED) ──────────\n")
    if not unique_uncertain:
        report_f.write("No uncertain loop intent cases found.\n\n")
    else:
        for i, c in enumerate(unique_uncertain, 1):
            report_f.write(f"[{i}] FILE: {c['file']}\n    REASON: {c['reason']}\n    PROB:   {c['probability']}\n    LOG:    {c['log']}\n")
            if c["matching_folders"]:
                report_f.write(f"    FOLDERS: {', '.join(c['matching_folders'])}\n")
            report_f.write("-" * 40 + "\n")
        report_f.write("\n")

    report_f.write("── MODERN TO LEGACY CONVERSIONS ───────────────────────────────\n")
    if not unique_results:
        report_f.write("No conversions found.\n")
    else:
        for i, r in enumerate(unique_results, 1):
            report_f.write(f"[{i}] SOURCE: {r['source']}\n    TARGET: {r['target']}\n    STATUS: {r['status']}\n    INFO:   {r['details']}\n    LOG:    {r['log']}\n")
            report_f.write("-" * 40 + "\n")

    return len(unique_results), len(unique_uncertain)

# ---------------------------------------------------------------------------
# Main Execution
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="MFB Conversion Analyzer & Integrity Verifier")
    parser.add_argument("logs", nargs="*", help="Log files or directories to scan.")
    parser.add_argument("--verify", nargs="+", help="Source and/or optimized directories for integrity check (auto-detects if one provided).")
    parser.add_argument("-o", "--output", help="Custom output report path.")

    args = parser.parse_args()

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    output_report = args.output if args.output else f"logs/diagnostic_report_{timestamp}.txt"
    os.makedirs(os.path.dirname(output_report), exist_ok=True) if os.path.dirname(output_report) else None

    with open(output_report, "w", encoding="utf-8") as report_f:
        report_f.write("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n")
        report_f.write("      MODERN FORMAT BOOST - DIAGNOSTIC ANALYSIS REPORT\n")
        report_f.write(f"      Generated at: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}\n")
        report_f.write("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n")

        # 1. Integrity Check
        source_dir_context = None
        if args.verify:
            resolved = resolve_verify_dirs(args.verify)
            if resolved:
                src, opt = resolved
                source_dir_context = src
                run_integrity_check(src, opt, report_f)
            else:
                report_f.write(f"❌ Error: Could not resolve paired directory for {args.verify[0]}\n\n")

        # 2. Log Analysis
        if args.logs:
            conv_count, unc_count = parse_logs(args.logs, report_f, filter_dir=source_dir_context)
            print(f"📈 Total conversion events: {conv_count}")
            print(f"🔭 Uncertain loop cases: {unc_count}")

    print(f"📊 Full report generated: {output_report}")


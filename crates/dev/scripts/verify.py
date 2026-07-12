#!/usr/bin/env python3
"""Modern Format Boost - Conversion Analyzer & Integrity Verifier

Combines log-based conversion analysis with filesystem-level integrity checking.
Extracts modern format conversions, loop intent edge cases (Uncertain/KNN Bypass),
and verifies that every source file has a corresponding optimized output.

Usage:
    # 1. Log analysis (default: ~/.modern_format_boost/logs or pass log paths)
    python3 verify.py

    # 2. Integrity verification (auto-detects paired source/optimized folder)
    python3 verify.py --verify /path/to/MyPhotos_optimized

    # 3. Integrity verification (explicitly specify both directories)
    python3 verify.py --verify /path/to/Source /path/to/Optimized

    # 4. Combined analysis (log scanning + auto-detecting verification)
    python3 verify.py ~/.modern_format_boost/logs/ --verify /path/to/MyPhotos

    # 5. Combined analysis (log scanning + explicit dual-directory verification)
    python3 verify.py ~/.modern_format_boost/logs/ --verify /path/to/Source /path/to/Optimized

    # 6. With session routing audit + bundle run logs (cross-layer reconciliation)
    python3 verify.py ~/.modern_format_boost/logs/Bundle_*/ --verify src/ opt/ \\
        --session-audit ~/.modern_format_boost/logs/verbose_*.log --mode both

Media routing / animation rules live in media_scope.py (shared with drag_and_drop_processor.py).
Rust img/vid emit target mfb::audit lines that verify can grep from img_run_*.log / vid_run_*.log.
"""

import argparse
import hashlib
import json
import os
import re
import sys
import unicodedata
from datetime import datetime
from pathlib import Path

from mfb_log_paths import unified_log_dir
from media_scope import (
    MediaProbeError,
    SKIP_EXTS,
    classify_media_owner,
    classify_missing_entry,
    detect_true_format,
    load_rust_outcomes_from_logs,
    load_session_pipeline_exits,
    load_session_preserve_handoff,
    load_session_routing,
    reconcile_handoff,
    session_handoff_preserve_was_declined,
    true_format_matches_processing_mode,
)
from mfb_ui_tokens import pick_symbol

_REPORT_ERR = pick_symbol("❌", "[ERROR]")
_REPORT_WARN = pick_symbol("⚠️", "[WARN]")

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
# Integrity Verification Logic (format truth mirrors Rust format_detect.rs)
# ---------------------------------------------------------------------------


def is_media_file(path: Path) -> bool:
    """Check media truth from content, never from extension alone."""
    if path.name.startswith("."):
        return False
    ext = path.suffix.lower()
    if ext in SKIP_EXTS:
        return False
    return detect_true_format(path) != "unknown"


def file_content_hash(
    path: Path, chunk_size: int = 65536
) -> tuple[str | None, str | None]:
    """Compute SHA-256 of first 64KB for fast identity comparison."""
    h = hashlib.sha256()
    try:
        with open(path, "rb") as f:
            h.update(f.read(chunk_size))
    except OSError as exc:
        return None, str(exc)
    return h.hexdigest()[:16], None


def collect_media_files(
    directory: Path, processing_mode: str = "both"
) -> dict[str, list[Path]]:
    """Walk directory and collect all media files, tracking potential collisions."""
    result: dict[str, list[Path]] = {}
    for root, _, files in os.walk(directory):
        for fname in files:
            full = Path(root) / fname
            if full.name.startswith(".") or full.suffix.lower() in SKIP_EXTS:
                continue
            try:
                true_format = detect_true_format(full)
                if true_format == "unknown":
                    continue
                if not true_format_matches_processing_mode(
                    full, true_format, processing_mode
                ):
                    continue
            except (OSError, MediaProbeError) as exc:
                raise RuntimeError(
                    f"media routing probe failed for {full}: {exc}"
                ) from exc
            try:
                rel = full.relative_to(directory)
                # The 'key' is the relative path without suffix, normalized to Unicode NFC and casefolded.
                # This helps match 'Folder/Img.jpg' with 'folder/img.jxl' across unicode normalization differences.
                raw_key = str(rel.with_suffix(""))
                # Normalize unicode (NFC) and casefold for reliable cross-platform comparisons.
                key = unicodedata.normalize("NFC", raw_key).casefold().strip()
                if key not in result:
                    result[key] = []
                result[key].append(full)
            except ValueError as exc:
                raise RuntimeError(
                    f"media routing relative path failed for {full} under {directory}: {exc}"
                ) from exc
    return result


def is_true_jpeg_file(path: Path) -> bool:
    return detect_true_format(path) == "jpeg"


def collect_regular_files(directory: Path) -> list[Path]:
    files = []
    for root, _, names in os.walk(directory):
        for name in names:
            full = Path(root) / name
            if full.is_file():
                files.append(full)
    return sorted(files)


def mfb_home_root() -> Path:
    env_root = (os.environ.get("MFB_HOME_ROOT") or "").strip()
    if env_root:
        return Path(env_root).expanduser()
    home = (os.environ.get("HOME") or "").strip()
    if home:
        return Path(home).expanduser() / ".modern_format_boost"
    return Path.home() / ".modern_format_boost"


def same_path(left: Path, right: Path) -> bool:
    try:
        return left.resolve(strict=False) == right.resolve(strict=False)
    except OSError:
        return left.absolute() == right.absolute()


def fast_img_marker_candidates(optimized_dir: Path) -> list[Path]:
    paths = []
    marker_dir = mfb_home_root() / "fast_img" / "markers"
    if marker_dir.is_dir():
        paths.extend(sorted(marker_dir.glob("*.json")))
    paths.append(optimized_dir / ".mfb_wc")
    return paths


def load_fast_img_marker_for_optimized(
    optimized_dir: Path,
) -> tuple[dict | None, Path | None, str | None]:
    for path in fast_img_marker_candidates(optimized_dir):
        if not path.is_file():
            continue
        try:
            marker = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            if path.name == ".mfb_wc":
                return None, path, f"fast-img marker unreadable: {path}: {exc}"
            continue
        working_copy = marker.get("working_copy")
        if not isinstance(working_copy, str):
            continue
        if not same_path(Path(working_copy), optimized_dir):
            continue
        src_jpeg_count = marker.get("src_jpeg_count")
        if type(src_jpeg_count) is not int or src_jpeg_count < 0:
            return None, path, f"fast-img marker has invalid src_jpeg_count: {path}"
        skipped_sources = marker.get("skipped_sources", {})
        if not isinstance(skipped_sources, dict):
            return None, path, f"fast-img marker has invalid skipped_sources: {path}"
        failed_sources = marker.get("failed_sources", {})
        if not isinstance(failed_sources, dict):
            return None, path, f"fast-img marker has invalid failed_sources: {path}"
        for rel, entry in skipped_sources.items():
            if not isinstance(rel, str) or not isinstance(entry, dict):
                return (
                    None,
                    path,
                    f"fast-img marker has invalid skipped source entry: {path}",
                )
            if not isinstance(entry.get("src"), str) or not entry.get("src"):
                return (
                    None,
                    path,
                    f"fast-img marker skipped source missing src hash: {path}",
                )
            if (
                not isinstance(entry.get("reason"), str)
                or not entry.get("reason").strip()
            ):
                return (
                    None,
                    path,
                    f"fast-img marker skipped source missing reason: {path}",
                )
        for rel, entry in failed_sources.items():
            if not isinstance(rel, str) or not isinstance(entry, dict):
                return (
                    None,
                    path,
                    f"fast-img marker has invalid failed source entry: {path}",
                )
            if not isinstance(entry.get("src"), str) or not entry.get("src"):
                return (
                    None,
                    path,
                    f"fast-img marker failed source missing src hash: {path}",
                )
            if (
                not isinstance(entry.get("reason"), str)
                or not entry.get("reason").strip()
            ):
                return (
                    None,
                    path,
                    f"fast-img marker failed source missing reason: {path}",
                )
        return marker, path, None
    return None, None, "fast-img marker missing for optimized directory"


def run_fast_img_delivery_check(
    source_dir: Path,
    optimized_dir: Path,
    report_f,
    processing_mode: str = "images_only",
):
    """Verify fast-img's post-delivery invariant: source true JPEGs gone, output JXL-only."""
    report_f.write("── FAST-IMG DELIVERY VERIFICATION ─────────────────────────────\n")
    report_f.write(f"Source:    {source_dir}\n")
    report_f.write(f"Optimized: {optimized_dir}\n\n")

    if not source_dir.is_dir() or not optimized_dir.is_dir():
        report_f.write(
            f"{_REPORT_ERR} Error: Source or Optimized directory missing.\n\n"
        )
        return None

    source_true_jpegs = []
    source_probe_errors = []
    for path in collect_regular_files(source_dir):
        try:
            if is_true_jpeg_file(path):
                source_true_jpegs.append(path)
        except (OSError, MediaProbeError) as exc:
            source_probe_errors.append((path, exc))
    optimized_jxl = []
    non_jxl_outputs = []
    optimized_probe_errors = []
    for path in collect_regular_files(optimized_dir):
        if path.name.startswith("."):
            continue
        try:
            true_format = detect_true_format(path)
        except (OSError, MediaProbeError) as exc:
            optimized_probe_errors.append((path, exc))
            continue
        if true_format == "jxl":
            optimized_jxl.append(path)
        else:
            non_jxl_outputs.append((path, true_format))
    marker, marker_path, marker_error = load_fast_img_marker_for_optimized(
        optimized_dir
    )
    recorded_source_jpegs = marker.get("src_jpeg_count") if marker else 0
    skipped_sources = marker.get("skipped_sources", {}) if marker else {}
    failed_sources = marker.get("failed_sources", {}) if marker else {}
    skipped_source_rels = set(skipped_sources)
    failed_source_rels = set(failed_sources)

    tier2_recorded = 0
    tier2_verified_deleted = 0
    tier2_unexpected_remaining = []
    tier2_missing_proof = []
    if marker:
        tier2_assets = marker.get("tier2_imported_assets", [])
        tier2_recorded = len(tier2_assets)
        for item in tier2_assets:
            rel = item.get("rel_path")
            if not rel:
                continue
            photos_uuid = item.get("photos_uuid")
            source_path = source_dir / rel
            exists = source_path.is_file()

            if exists:
                tier2_unexpected_remaining.append(rel)
            elif photos_uuid:
                tier2_verified_deleted += 1
            else:
                tier2_missing_proof.append(rel)
    skipped_sources_present = []
    failed_sources_present = []
    unexpected_source_true_jpegs = []
    for path in source_true_jpegs:
        rel = path.relative_to(source_dir).as_posix()
        if rel in skipped_source_rels:
            skipped_sources_present.append(path)
        elif rel in failed_source_rels:
            failed_sources_present.append(path)
        else:
            unexpected_source_true_jpegs.append(path)
    skipped_sources_missing = sorted(
        rel for rel in skipped_source_rels if not (source_dir / rel).is_file()
    )
    failed_sources_missing = sorted(
        rel for rel in failed_source_rels if not (source_dir / rel).is_file()
    )
    expected_optimized_jxl = max(
        recorded_source_jpegs - len(skipped_sources) - len(failed_sources), 0
    )

    optimized_size = 0
    for path in optimized_jxl:
        try:
            optimized_size += path.stat().st_size
        except OSError as exc:
            optimized_probe_errors.append((path, exc))

    report_f.write(f"Scope:           {processing_mode}\n")
    if unexpected_source_true_jpegs:
        report_f.write(
            f"--- Unexpected source true JPEGs still present ({len(unexpected_source_true_jpegs)}) ---\n"
        )
        for path in unexpected_source_true_jpegs:
            report_f.write(f"  ✗ {path.relative_to(source_dir)}\n")
        report_f.write("\n")
    if skipped_sources_present:
        report_f.write(
            f"--- Recorded skipped source JPEGs retained ({len(skipped_sources_present)}) ---\n"
        )
        for path in skipped_sources_present:
            report_f.write(f"  = {path.relative_to(source_dir)}\n")
        report_f.write("\n")
    if failed_sources_present:
        report_f.write(
            f"--- Recorded failed source JPEGs retained ({len(failed_sources_present)}) ---\n"
        )
        for path in failed_sources_present:
            report_f.write(f"  ! {path.relative_to(source_dir)}\n")
        report_f.write("\n")
    if skipped_sources_missing:
        report_f.write(
            f"--- Recorded skipped source JPEGs missing ({len(skipped_sources_missing)}) ---\n"
        )
        for rel in skipped_sources_missing:
            report_f.write(f"  ! {rel}\n")
        report_f.write("\n")
    if failed_sources_missing:
        report_f.write(
            f"--- Recorded failed source JPEGs missing ({len(failed_sources_missing)}) ---\n"
        )
        for rel in failed_sources_missing:
            report_f.write(f"  ! {rel}\n")
        report_f.write("\n")

    if source_probe_errors:
        report_f.write(
            f"--- Source format probe errors ({len(source_probe_errors)}) ---\n"
        )
        for path, exc in source_probe_errors:
            report_f.write(f"  ! {path.relative_to(source_dir)}: {exc}\n")
        report_f.write("\n")

    if non_jxl_outputs:
        report_f.write(f"--- Non-JXL optimized files ({len(non_jxl_outputs)}) ---\n")
        for path, true_format in non_jxl_outputs:
            report_f.write(
                f"  + {path.relative_to(optimized_dir)} [true_format={true_format}]\n"
            )
        report_f.write("\n")

    if optimized_probe_errors:
        report_f.write(
            f"--- Optimized format probe errors ({len(optimized_probe_errors)}) ---\n"
        )
        for path, exc in optimized_probe_errors:
            report_f.write(f"  ! {path.relative_to(optimized_dir)}: {exc}\n")
        report_f.write("\n")

    if tier2_unexpected_remaining:
        report_f.write(
            f"--- Tier-2 modern lossy files remained under source ({len(tier2_unexpected_remaining)}) ---\n"
        )
        for rel in tier2_unexpected_remaining:
            report_f.write(f"  ✗ {rel}\n")
        report_f.write("\n")

    if tier2_missing_proof:
        report_f.write(
            f"--- Tier-2 modern lossy files deleted without Photos/iCloud proof ({len(tier2_missing_proof)}) ---\n"
        )
        for rel in tier2_missing_proof:
            report_f.write(f"  ! {rel}\n")
        report_f.write("\n")

    if not optimized_jxl:
        if expected_optimized_jxl == 0 and (skipped_sources or failed_sources):
            report_f.write(
                "No JXL outputs expected: all recorded source JPEGs were skipped/failed and retained.\n\n"
            )
        else:
            report_f.write("⚠️ No JXL outputs found in optimized directory.\n\n")
    if marker_error:
        report_f.write(f"{_REPORT_ERR} {marker_error}\n\n")
    elif marker_path is not None:
        report_f.write(f"Marker:          {marker_path}\n")

    integrity_failures = (
        len(unexpected_source_true_jpegs)
        + len(skipped_sources_missing)
        + len(failed_sources_missing)
        + len(source_probe_errors)
        + len(non_jxl_outputs)
        + len(optimized_probe_errors)
        + len(tier2_unexpected_remaining)
        + len(tier2_missing_proof)
    )
    if not optimized_jxl and expected_optimized_jxl > 0:
        integrity_failures += 1
    if marker_error:
        integrity_failures += 1
    if marker is not None and len(optimized_jxl) != expected_optimized_jxl:
        integrity_failures += 1

    count_status_label = (
        "FAST_IMG_JXL_ONLY_DELIVERY" if integrity_failures == 0 else None
    )
    count_matches = integrity_failures == 0

    report_f.write(f"Recorded source JPEGs:       {recorded_source_jpegs}\n")
    report_f.write(f"Recorded skipped JPEGs:      {len(skipped_sources)}\n")
    report_f.write(f"Recorded failed JPEGs:       {len(failed_sources)}\n")
    report_f.write(f"Expected optimized JXLs:     {expected_optimized_jxl}\n")
    report_f.write(f"Optimized JXL files:         {len(optimized_jxl)}\n")
    report_f.write(f"Source true JPEGs remaining: {len(source_true_jpegs)}\n")
    report_f.write(f"Source probe errors:         {len(source_probe_errors)}\n")
    report_f.write(f"Optimized probe errors:      {len(optimized_probe_errors)}\n")
    report_f.write(f"Non-JXL optimized files:     {len(non_jxl_outputs)}\n")
    if tier2_recorded > 0:
        report_f.write(f"Recorded tier-2 lossy files: {tier2_recorded}\n")
        report_f.write(f"Verified tier-2 deleted:     {tier2_verified_deleted}\n")
    report_f.write(
        "Count status:    "
        f"{count_status_label if count_status_label else 'FAST_IMG_DELIVERY_MISMATCH'}\n\n"
    )

    return {
        "source": str(source_dir),
        "optimized": str(optimized_dir),
        "optimized_path_label": "Optimized",
        "scope": processing_mode,
        "source_files_label": "Recorded source JPEGs",
        "source_count_source": (
            "fast_img_marker" if marker is not None else "missing_fast_img_marker"
        ),
        "fast_img_marker": str(marker_path) if marker_path else None,
        "optimized_files_label": "Optimized JXL files",
        "source_files": recorded_source_jpegs,
        "source_remaining_label": "Source true JPEGs remaining",
        "source_remaining_files": len(source_true_jpegs),
        "source_probe_errors": len(source_probe_errors),
        "optimized_probe_errors": len(optimized_probe_errors),
        "optimized_files": len(optimized_jxl),
        "skipped_sources": len(skipped_sources),
        "failed_sources": len(failed_sources),
        "count_delta": len(optimized_jxl) - expected_optimized_jxl,
        "count_matches_with_handoff": count_matches,
        "count_fully_explained": count_matches,
        "count_status_label": count_status_label,
        "tier2_recorded": tier2_recorded,
        "tier2_verified_deleted": tier2_verified_deleted,
        "explained_gaps": len(skipped_sources) + len(failed_sources),
        "expected_count_delta": 0,
        "matched": len(optimized_jxl),
        "ambiguous": 0,
        "missing": 0,
        "pipeline_handoff": 0,
        "vid_pipeline_failed": 0,
        "vid_pipeline_unverified": 0,
        "missing_total": 0,
        "extra": len(non_jxl_outputs) + len(optimized_probe_errors),
        "mismatched_types": 0,
        "source_total_size": 0,
        "optimized_total_size": optimized_size,
        "integrity_failures": integrity_failures,
        "has_warnings": integrity_failures > 0,
        "integrity_kind": "fast-img delivery",
    }


def _relative_stem_key(root: Path, path: Path) -> str:
    rel = path.relative_to(root)
    raw_key = str(rel.with_suffix(""))
    return unicodedata.normalize("NFC", raw_key).casefold().strip()


RESTORE_JPEG_MANIFEST_NAME = ".mfb_restore_jpeg_manifest.tsv"


def _restore_jpeg_manifest_key(rel_text: str) -> str:
    raw_key = str(Path(rel_text).with_suffix(""))
    return unicodedata.normalize("NFC", raw_key).casefold().strip()


def _decode_restore_manifest_hex(field: str, *, line_no: int, field_name: str) -> str:
    try:
        return bytes.fromhex(field).decode("utf-8")
    except (ValueError, UnicodeDecodeError) as exc:
        raise ValueError(
            f"line {line_no}: invalid {field_name} hex/UTF-8 field"
        ) from exc


def _safe_restore_manifest_relpath(
    rel_text: str, *, line_no: int, field_name: str
) -> Path:
    if not rel_text or "\0" in rel_text:
        raise ValueError(f"line {line_no}: empty or invalid {field_name}")
    rel_path = Path(rel_text)
    if rel_path.is_absolute() or any(part == ".." for part in rel_path.parts):
        raise ValueError(f"line {line_no}: unsafe {field_name}: {rel_text}")
    return rel_path


def _path_exists_for_restore_manifest(path: Path) -> tuple[bool, str | None]:
    try:
        path.stat()
    except FileNotFoundError:
        return False, None
    except OSError as exc:
        return False, str(exc)
    return True, None


def restore_jpeg_xmp_sidecar_candidates(
    source_path: Path,
) -> tuple[list[Path], list[str]]:
    candidates: list[Path] = []
    errors: list[str] = []

    def add_candidate(path: Path) -> None:
        if path not in candidates:
            candidates.append(path)

    suffix = source_path.suffix
    if suffix:
        add_candidate(source_path.with_suffix(f"{suffix}.xmp"))
        add_candidate(source_path.with_suffix(f"{suffix}.XMP"))
    add_candidate(source_path.with_suffix(".xmp"))
    add_candidate(source_path.with_suffix(".XMP"))

    parent = source_path.parent
    source_stem = source_path.stem.lower()
    source_ext = suffix[1:].lower() if suffix.startswith(".") else suffix.lower()
    source_compound = f"{source_stem}.{source_ext}" if source_ext else source_stem
    try:
        entries = list(parent.iterdir())
    except FileNotFoundError:
        entries = []
    except OSError as exc:
        errors.append(f"manifest deleted source sidecar scan failed: {parent}: {exc}")
        entries = []
    for path in entries:
        try:
            is_file = path.is_file()
        except OSError as exc:
            errors.append(f"manifest deleted source sidecar stat failed: {path}: {exc}")
            continue
        if not is_file or path.suffix.lower() != ".xmp":
            continue
        stem = path.stem.lower()
        if stem == source_stem or stem == source_compound:
            add_candidate(path)
    return candidates, errors


def load_restore_jpeg_manifest(
    restored_dir: Path,
) -> tuple[list[dict[str, str]], list[str]]:
    manifest = restored_dir / RESTORE_JPEG_MANIFEST_NAME
    if not manifest.is_file():
        return [], []
    try:
        lines = manifest.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        return [], [f"restore manifest unreadable: {manifest}: {exc}"]

    records: list[dict[str, str]] = []
    errors: list[str] = []
    for line_no, raw_line in enumerate(lines, start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("source_rel_hex\toutput_rel_hex\t"):
            continue
        parts = line.split("\t")
        if len(parts) != 5:
            errors.append(f"line {line_no}: expected 5 TSV fields, got {len(parts)}")
            continue
        source_rel_hex, output_rel_hex, source_hash, output_hash, source_deleted = parts
        try:
            source_rel = _decode_restore_manifest_hex(
                source_rel_hex, line_no=line_no, field_name="source_rel"
            )
            output_rel = _decode_restore_manifest_hex(
                output_rel_hex, line_no=line_no, field_name="output_rel"
            )
            _safe_restore_manifest_relpath(
                source_rel, line_no=line_no, field_name="source_rel"
            )
            _safe_restore_manifest_relpath(
                output_rel, line_no=line_no, field_name="output_rel"
            )
        except ValueError as exc:
            errors.append(str(exc))
            continue
        if source_deleted != "true":
            errors.append(f"line {line_no}: source_deleted must be true")
            continue
        if not source_hash.strip() or not output_hash.strip():
            errors.append(f"line {line_no}: missing manifest hash field")
            continue
        records.append(
            {
                "source_rel": source_rel,
                "output_rel": output_rel,
                "source_blake3": source_hash,
                "output_blake3": output_hash,
            }
        )
    return records, errors


def run_fast_img_restore_check(
    source_dir: Path,
    restored_dir: Path,
    report_f,
):
    """Verify Fastmode Img restore-jpeg: true JXL inputs decode back to true JPEGs."""

    report_f.write(f"Source:    {source_dir}\n")
    report_f.write(f"Restored:  {restored_dir}\n\n")

    if not source_dir.is_dir() or not restored_dir.is_dir():
        report_f.write(
            f"{_REPORT_ERR} Error: Source or restored directory missing.\n\n"
        )
        return None

    source_jxl: dict[str, Path] = {}
    source_probe_errors = []
    for path in collect_regular_files(source_dir):
        if path.name.startswith(".") or path.suffix.lower() in SKIP_EXTS:
            continue
        try:
            true_format = detect_true_format(path)
        except (OSError, MediaProbeError) as exc:
            source_probe_errors.append((path, exc))
            continue
        if true_format == "jxl":
            source_jxl[_relative_stem_key(source_dir, path)] = path

    restored_jpeg: dict[str, Path] = {}
    restored_probe_errors = []
    non_jpeg_outputs = []
    for path in collect_regular_files(restored_dir):
        if path.name.startswith(".") or path.suffix.lower() in SKIP_EXTS:
            continue
        try:
            true_format = detect_true_format(path)
        except (OSError, MediaProbeError) as exc:
            restored_probe_errors.append((path, exc))
            continue
        if true_format == "jpeg":
            restored_jpeg[_relative_stem_key(restored_dir, path)] = path
        else:
            non_jpeg_outputs.append((path, true_format))

    manifest_records, restore_manifest_errors = load_restore_jpeg_manifest(restored_dir)
    manifest_deleted_sources: dict[str, dict[str, str]] = {}
    for record in manifest_records:
        source_rel = record["source_rel"]
        output_rel = record["output_rel"]
        source_key = _restore_jpeg_manifest_key(source_rel)
        output_key = _restore_jpeg_manifest_key(output_rel)
        if source_key != output_key:
            restore_manifest_errors.append(
                f"manifest key mismatch: source={source_rel} output={output_rel}"
            )
            continue
        source_path = source_dir / _safe_restore_manifest_relpath(
            source_rel, line_no=0, field_name="source_rel"
        )
        output_path = restored_dir / _safe_restore_manifest_relpath(
            output_rel, line_no=0, field_name="output_rel"
        )
        source_exists, source_exists_error = _path_exists_for_restore_manifest(
            source_path
        )
        if source_exists_error is not None:
            restore_manifest_errors.append(
                f"manifest deleted source stat failed: {source_path.relative_to(source_dir)}: {source_exists_error}"
            )
            continue
        if source_exists:
            restore_manifest_errors.append(
                f"manifest claims deleted source still exists: {source_path.relative_to(source_dir)}"
            )
            continue
        sidecar_candidates, sidecar_scan_errors = restore_jpeg_xmp_sidecar_candidates(
            source_path
        )
        if sidecar_scan_errors:
            restore_manifest_errors.extend(sidecar_scan_errors)
            continue
        sidecar_leftovers = []
        for sidecar in sidecar_candidates:
            sidecar_exists, sidecar_exists_error = _path_exists_for_restore_manifest(
                sidecar
            )
            if sidecar_exists_error is not None:
                restore_manifest_errors.append(
                    f"manifest deleted source sidecar stat failed: {sidecar.relative_to(source_dir)}: {sidecar_exists_error}"
                )
                continue
            if sidecar_exists:
                sidecar_leftovers.append(sidecar)
        if sidecar_leftovers:
            joined = ", ".join(
                str(path.relative_to(source_dir)) for path in sidecar_leftovers
            )
            restore_manifest_errors.append(
                f"manifest deleted source left XMP sidecar: {joined}"
            )
            continue
        if not output_path.is_file():
            restore_manifest_errors.append(
                f"manifest restored output missing: {output_path.relative_to(restored_dir)}"
            )
            continue
        try:
            output_format = detect_true_format(output_path)
        except (OSError, MediaProbeError) as exc:
            restored_probe_errors.append((output_path, exc))
            continue
        if output_format != "jpeg":
            restore_manifest_errors.append(
                f"manifest restored output is not true JPEG: {output_path.relative_to(restored_dir)} [true_format={output_format}]"
            )
            continue
        if source_key in manifest_deleted_sources:
            restore_manifest_errors.append(
                f"duplicate manifest source key: {source_rel}"
            )
            continue
        manifest_deleted_sources[source_key] = record

    expected_keys = set(source_jxl) | set(manifest_deleted_sources)
    matched_keys = sorted(expected_keys & set(restored_jpeg))
    missing_keys = sorted(expected_keys - set(restored_jpeg))
    extra_keys = sorted(set(restored_jpeg) - expected_keys)
    non_jpeg_matched_keys = {
        _relative_stem_key(restored_dir, path)
        for path, _true_format in non_jpeg_outputs
    } & expected_keys

    report_f.write("Restore mode:   JXL -> JPEG via djxl\n")
    report_f.write("Scope:          fast_img_restore\n")

    if missing_keys:
        report_f.write(f"--- Missing restored JPEG outputs ({len(missing_keys)}) ---\n")
        for key in missing_keys:
            if key in source_jxl:
                report_f.write(f"  ! {source_jxl[key].relative_to(source_dir)}\n")
            else:
                report_f.write(f"  ! {manifest_deleted_sources[key]['source_rel']}\n")
        report_f.write("\n")

    if extra_keys:
        report_f.write(f"--- Extra restored JPEG outputs ({len(extra_keys)}) ---\n")
        for key in extra_keys:
            report_f.write(f"  + {restored_jpeg[key].relative_to(restored_dir)}\n")
        report_f.write("\n")

    if non_jpeg_outputs:
        report_f.write(f"--- Non-JPEG restored outputs ({len(non_jpeg_outputs)}) ---\n")
        for path, true_format in non_jpeg_outputs:
            report_f.write(
                f"  x {path.relative_to(restored_dir)} [true_format={true_format}]\n"
            )
        report_f.write("\n")

    if source_probe_errors:
        report_f.write(
            f"--- Source JXL format probe errors ({len(source_probe_errors)}) ---\n"
        )
        for path, exc in source_probe_errors:
            report_f.write(f"  ! {path.relative_to(source_dir)}: {exc}\n")
        report_f.write("\n")

    if restored_probe_errors:
        report_f.write(
            f"--- Restored JPEG format probe errors ({len(restored_probe_errors)}) ---\n"
        )
        for path, exc in restored_probe_errors:
            report_f.write(f"  ! {path.relative_to(restored_dir)}: {exc}\n")
        report_f.write("\n")

    if restore_manifest_errors:
        report_f.write(
            f"--- Restore manifest errors ({len(restore_manifest_errors)}) ---\n"
        )
        for error in restore_manifest_errors:
            report_f.write(f"  ! {error}\n")
        report_f.write("\n")

    source_total_size = 0
    restored_total_size = 0
    for key in matched_keys:
        try:
            if key in source_jxl:
                source_total_size += source_jxl[key].stat().st_size
            restored_total_size += restored_jpeg[key].stat().st_size
        except OSError as exc:
            restored_probe_errors.append((restored_jpeg[key], exc))

    integrity_failures = (
        len(missing_keys)
        + len(extra_keys)
        + len(non_jpeg_outputs)
        + len(source_probe_errors)
        + len(restored_probe_errors)
        + len(restore_manifest_errors)
    )
    count_status_label = "FAST_IMG_JPEG_RESTORE" if integrity_failures == 0 else None

    report_f.write(f"Source JXL files:           {len(expected_keys)}\n")
    report_f.write(f"Source remaining JXL files: {len(source_jxl)}\n")
    report_f.write(
        f"Manifest verified deleted source JXLs: {len(manifest_deleted_sources)}\n"
    )
    report_f.write(f"Restored JPEG files:        {len(restored_jpeg)}\n")
    report_f.write(f"Source probe errors:        {len(source_probe_errors)}\n")
    report_f.write(f"Restored probe errors:      {len(restored_probe_errors)}\n")
    report_f.write(f"Restore manifest errors:    {len(restore_manifest_errors)}\n")
    report_f.write(f"Non-JPEG restored outputs:  {len(non_jpeg_outputs)}\n")
    report_f.write(
        "Count status:    "
        f"{count_status_label if count_status_label else 'FAST_IMG_JPEG_RESTORE_MISMATCH'}\n\n"
    )

    return {
        "source": str(source_dir),
        "optimized": str(restored_dir),
        "optimized_path_label": "Restored",
        "scope": "fast_img_restore",
        "source_files_label": "Source JXL files",
        "optimized_files_label": "Restored JPEG files",
        "source_files": len(expected_keys),
        "source_remaining_files": len(source_jxl),
        "verified_deleted_sources": len(manifest_deleted_sources),
        "optimized_files": len(restored_jpeg),
        "source_probe_errors": len(source_probe_errors),
        "optimized_probe_errors": len(restored_probe_errors),
        "restore_manifest_errors": len(restore_manifest_errors),
        "count_delta": len(restored_jpeg) - len(expected_keys),
        "count_matches_with_handoff": integrity_failures == 0,
        "count_fully_explained": integrity_failures == 0,
        "count_status_label": count_status_label,
        "explained_gaps": len(missing_keys) + len(extra_keys) + len(non_jpeg_outputs),
        "expected_count_delta": 0,
        "matched": len(matched_keys),
        "ambiguous": 0,
        "missing": len(missing_keys),
        "pipeline_handoff": 0,
        "vid_pipeline_failed": 0,
        "vid_pipeline_unverified": 0,
        "missing_total": len(missing_keys),
        "extra": len(extra_keys),
        "mismatched_types": len(non_jpeg_matched_keys),
        "source_total_size": source_total_size,
        "optimized_total_size": restored_total_size,
        "integrity_failures": integrity_failures,
        "has_warnings": integrity_failures > 0,
        "integrity_kind": "fast-img restore",
    }


def choose_primary_output(paths):
    # Choose canonical optimized output by true content format priority.
    priority = [
        "jxl",
        "avif",
        "webp",
        "heic",
        "heif",
        "png",
        "jpeg",
        "gif",
        "mp4",
        "mov",
        "webm",
    ]
    detected: dict[Path, str] = {}
    for path in paths:
        try:
            detected[path] = detect_true_format(path)
        except OSError as exc:
            raise RuntimeError(
                f"primary output true-format probe failed for {path}: {exc}"
            ) from exc
    for true_format in priority:
        for pp in paths:
            if detected.get(pp) == true_format:
                return pp
    try:
        return min(paths, key=lambda pp: pp.stat().st_size)
    except OSError as exc:
        raise RuntimeError(f"primary output size probe failed: {exc}") from exc


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
            source_name = given.name[: -len(suffix)]
            candidate = given.parent / source_name
            if candidate.is_dir():
                return candidate, given

    # Case 2: given path is the source → look for _optimized sibling
    for suffix in optimized_suffixes:
        candidate = given.parent / (given.name + suffix)
        if candidate.is_dir():
            return given, candidate

    return None


def _collect_bundle_run_logs(log_dir: Path) -> list[Path]:
    """img/vid run logs and jsonl traces from a Bundle_* folder or logs/ root."""
    if not log_dir.is_dir():
        return []
    paths: list[Path] = []
    for pattern in (
        "img_run_*.log",
        "vid_run_*.log",
        "img_*.jsonl",
        "vid_*.jsonl",
        "**/img_run_*.log",
        "**/vid_run_*.log",
        "**/img_*.jsonl",
        "**/vid_*.jsonl",
    ):
        paths.extend(log_dir.glob(pattern))
    return sorted({p.resolve() for p in paths})


def run_integrity_check(
    source_dir: Path,
    optimized_dir: Path,
    report_f,
    processing_mode: str = "both",
    session_audit_paths: list[Path] | None = None,
    bundle_log_dir: Path | None = None,
    explicit_log_paths: list[Path] | None = None,
):
    """Perform integrity check and write results to the open report file handle."""
    report_f.write("── INTEGRITY VERIFICATION ─────────────────────────────────────\n")
    report_f.write(f"Source:    {source_dir}\n")
    report_f.write(f"Optimized: {optimized_dir}\n\n")

    if not source_dir.is_dir() or not optimized_dir.is_dir():
        report_f.write(
            f"{_REPORT_ERR} Error: Source or Optimized directory missing.\n\n"
        )
        return None

    source_files = collect_media_files(source_dir, processing_mode)
    optimized_files = collect_media_files(optimized_dir, processing_mode)

    # Collision Detection (Security hardening)
    src_collisions = {k: v for k, v in source_files.items() if len(v) > 1}
    opt_collisions = {k: v for k, v in optimized_files.items() if len(v) > 1}

    src_count = sum(len(v) for v in source_files.values())
    opt_count = sum(len(v) for v in optimized_files.values())

    report_f.write(f"Scope:           {processing_mode}\n")
    report_f.write(f"Source files:    {src_count}\n")
    report_f.write(f"Optimized files: {opt_count}\n")

    delta = opt_count - src_count

    audit_paths = list(session_audit_paths or [])
    if explicit_log_paths:
        log_paths = explicit_log_paths
    else:
        log_paths = _collect_bundle_run_logs(bundle_log_dir) if bundle_log_dir else []
    routing = load_session_routing(audit_paths) if audit_paths else {}
    rust_outcomes = (
        load_rust_outcomes_from_logs(log_paths, source_dir) if log_paths else {}
    )

    # Matching logic
    true_missing = []  # (key, path, note)
    pipeline_handoff = []  # expected gap (scope / vid static ignore / images_only anim)
    vid_pipeline_failed = []  # vid failed/skipped when transcode was required
    vid_pipeline_unverified = []  # video-route gap without mfb::audit
    matched = []  # List of (key, src_path, opt_path)
    extra = []
    ambiguous = []  # 1-to-N or N-to-1 matches

    # 1. Identify matches and missing
    for key, src_paths in sorted(source_files.items()):
        if key in optimized_files:
            opt_paths = optimized_files[key]
            # If it's a clean 1-to-1 match
            if len(src_paths) == 1 and len(opt_paths) == 1:
                matched.append((key, src_paths[0], opt_paths[0]))
            elif len(src_paths) == 1 and len(opt_paths) > 1:
                # multiple optimized outputs; pick canonical primary but keep ambiguity record
                primary = choose_primary_output(opt_paths)
                matched.append((key, src_paths[0], primary))
                ambiguous.append((key, src_paths, opt_paths))
            else:
                # N-to-M collision or ambiguity
                ambiguous.append((key, src_paths, opt_paths))
        else:
            for p in src_paths:
                category, note = classify_missing_entry(
                    p,
                    processing_mode,
                    routing=routing,
                    rust_outcomes=rust_outcomes,
                    source_dir=source_dir,
                )
                if category == "pipeline_handoff":
                    pipeline_handoff.append((key, p, note))
                elif category == "vid_pipeline_failed":
                    vid_pipeline_failed.append((key, p, note))
                elif category == "vid_pipeline_unverified":
                    vid_pipeline_unverified.append((key, p, note))
                else:
                    true_missing.append((key, p, note))

    # 2. Identify extras
    for key, opt_paths in sorted(optimized_files.items()):
        if key not in source_files:
            for p in opt_paths:
                extra.append((key, p))

    handoff_count = len(pipeline_handoff)
    vid_failed_count = len(vid_pipeline_failed)
    vid_unverified_count = len(vid_pipeline_unverified)
    explained_gaps = (
        len(true_missing) + handoff_count + vid_failed_count + vid_unverified_count
    )
    expected_count_delta = -handoff_count
    count_matches_with_handoff = delta == 0 or (
        vid_failed_count == 0 and delta == expected_count_delta
    )
    count_fully_explained = (
        delta != 0 and abs(delta) == explained_gaps and explained_gaps > 0
    )
    if count_matches_with_handoff:
        if handoff_count > 0 and delta == expected_count_delta:
            report_f.write(
                f"Count status:    MATCH ({handoff_count} expected handoff gap"
                f"{'' if handoff_count == 1 else 's'}; no data loss)\n"
            )
        else:
            report_f.write("Count status:    MATCH\n")
    elif count_fully_explained:
        direction = "more" if delta > 0 else "fewer"
        report_f.write(
            f"Count status:    EXPLAINED ({abs(delta)} {direction} in optimized; "
            f"all {explained_gaps} listed below — not a silent drop)\n"
        )
    else:
        direction = "more" if delta > 0 else "fewer"
        report_f.write(
            f"Count status:    MISMATCH ({abs(delta)} {direction} in optimized; "
            f"{explained_gaps} accounted in detail, "
            f"{abs(abs(delta) - explained_gaps)} still unexplained)\n"
        )

    report_f.write(f"Matched:         {len(matched)}\n")
    report_f.write(f"Ambiguous:       {len(ambiguous)} (Collisions detected!)\n")
    report_f.write(
        f"Missing:         {len(true_missing)} (static / img-owned, data-loss risk)\n"
    )
    report_f.write(
        f"Handoff gaps:    {len(pipeline_handoff)} "
        f"(expected gap: scope limit, vid static ignore, or images_only anim)\n"
    )
    report_f.write(
        f"Vid failures:    {len(vid_pipeline_failed)} "
        f"(both/videos_only: vid failed/skipped — real integrity gap)\n"
    )
    report_f.write(
        f"Vid unverified:  {len(vid_pipeline_unverified)} "
        f"(missing mfb::audit — attach session/bundle logs)\n"
    )
    report_f.write(f"Extra:           {len(extra)}\n\n")

    if src_collisions or opt_collisions or ambiguous:
        report_f.write(
            "── COLLISIONS & SAFETY WARNINGS ───────────────────────────────\n"
        )
        if src_collisions:
            report_f.write(
                "⚠️ WARNING: Duplicate source stems detected (Unsafe for 1-to-1 mapping):\n"
            )
            for key, paths in sorted(src_collisions.items()):
                hash_results = [file_content_hash(p) for p in paths]
                hash_errors = [err for _, err in hash_results if err is not None]
                hashes = [digest for digest, err in hash_results if err is None]
                if hash_errors:
                    label = f"[WARN] hash read failed for {len(hash_errors)} file(s)"
                else:
                    unique_h = len(set(hashes))
                    label = (
                        "IDENTICAL content"
                        if unique_h == 1
                        else f"{unique_h} DISTINCT files"
                    )
                report_f.write(f"  Key '{key}' maps to {len(paths)} files ({label}):\n")
                for p, (digest, err) in zip(paths, hash_results):
                    if err is None:
                        report_f.write(
                            f"    - {p.relative_to(source_dir)}  [sha256:{digest}]\n"
                        )
                    else:
                        report_f.write(
                            f"    - {p.relative_to(source_dir)}  [WARN] hash_read_failed: {err}\n"
                        )
            report_f.write("\n")

        if opt_collisions:
            report_f.write(
                "⚠️ WARNING: Duplicate optimized stems detected (Potential overwrites):\n"
            )
            for key, paths in sorted(opt_collisions.items()):
                hash_results = [file_content_hash(p) for p in paths]
                hash_errors = [err for _, err in hash_results if err is not None]
                hashes = [digest for digest, err in hash_results if err is None]
                if hash_errors:
                    label = f"[WARN] hash read failed for {len(hash_errors)} file(s)"
                else:
                    unique_h = len(set(hashes))
                    label = (
                        "IDENTICAL content"
                        if unique_h == 1
                        else f"{unique_h} DISTINCT files"
                    )
                report_f.write(
                    f"  Key '{key}' maps to {len(paths)} outputs ({label}):\n"
                )
                for p, (digest, err) in zip(paths, hash_results):
                    if err is None:
                        report_f.write(
                            f"    - {p.relative_to(optimized_dir)}  [sha256:{digest}]\n"
                        )
                    else:
                        report_f.write(
                            f"    - {p.relative_to(optimized_dir)}  [WARN] hash_read_failed: {err}\n"
                        )
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
    expected_image_to_gif = {"webp", "avif", "heic", "heif", "png"}
    for key, src_p, opt_p in matched:
        try:
            src_format = detect_true_format(src_p)
            opt_format = detect_true_format(opt_p)
            src_owner = classify_media_owner(src_p)
            opt_owner = classify_media_owner(opt_p)
        except (OSError, MediaProbeError) as exc:
            mismatched_types.append(
                (src_p, opt_p, "probe_error", "probe_error", str(exc))
            )
            continue
        # Expected compatibility transcodes should not be reported as suspicious mismatch.
        if src_format in expected_image_to_gif and opt_format == "gif":
            continue
        if src_owner != opt_owner:
            mismatched_types.append((src_p, opt_p, src_owner, opt_owner, ""))

    if mismatched_types:
        report_f.write(
            "── CONTENT CONSISTENCY WARNINGS ───────────────────────────────\n"
        )
        report_f.write(
            "⚠️ WARNING: Media type mismatch detected (Suspicious conversion):\n"
        )
        for src, opt, src_owner, opt_owner, detail in mismatched_types:
            s_type = (
                "Video"
                if src_owner == "video"
                else "Image"
                if src_owner == "image"
                else "Unknown"
            )
            o_type = (
                "Video"
                if opt_owner == "video"
                else "Image"
                if opt_owner == "image"
                else "Unknown"
            )
            suffix = f" [{detail}]" if detail else ""
            report_f.write(
                f"  - {src.name} ({s_type}) → {opt.name} ({o_type}){suffix}\n"
            )
        report_f.write("\n")

    if true_missing:
        report_f.write(
            f"--- Missing from Optimized — data-loss risk ({len(true_missing)}) ---\n"
        )
        for _, src_path, note in true_missing:
            rel = src_path.relative_to(source_dir)
            report_f.write(f"  ✗ {rel}\n")
            report_f.write(f"      {note}\n")
        report_f.write("\n")

    if vid_pipeline_failed:
        report_f.write(
            f"--- Vid pipeline failures — both/videos_only real gaps "
            f"({len(vid_pipeline_failed)}) ---\n"
        )
        for _, src_path, note in vid_pipeline_failed:
            rel = src_path.relative_to(source_dir)
            report_f.write(f"  {pick_symbol('✗', ('[FAIL]'))} {rel}\n")
            report_f.write(f"      {note}\n")
        report_f.write("\n")

    if vid_pipeline_unverified:
        report_f.write(
            f"--- Vid gaps unverified (no mfb::audit) ({len(vid_pipeline_unverified)}) ---\n"
        )
        for _, src_path, note in vid_pipeline_unverified:
            rel = src_path.relative_to(source_dir)
            report_f.write(f"  {pick_symbol('?', ('[AUDIT?]'))} {rel}\n")
            report_f.write(f"      {note}\n")
        report_f.write("\n")

    if pipeline_handoff:
        report_f.write(
            f"--- Handoff gaps — expected missing output ({len(pipeline_handoff)}) ---\n"
        )
        for _, src_path, note in pipeline_handoff:
            rel = src_path.relative_to(source_dir)
            report_f.write(f"  {pick_symbol('⚠', ('[WARN]'))} {rel}\n")
            report_f.write(f"      {note}\n")
        report_f.write("\n")

    if (pipeline_handoff or vid_pipeline_failed or vid_pipeline_unverified) and (
        audit_paths or log_paths
    ):
        preserved = load_session_preserve_handoff(audit_paths)
        preserve_declined = session_handoff_preserve_was_declined(audit_paths)
        pipeline_exits = load_session_pipeline_exits(audit_paths)
        report_f.write(
            "--- Cross-layer reconciliation (Python media_scope ↔ session ROUTED ↔ Rust mfb::audit) ---\n"
        )
        reconcile_entries = (
            pipeline_handoff + vid_pipeline_failed + vid_pipeline_unverified
        )
        for line in reconcile_handoff(
            reconcile_entries,
            routing,
            rust_outcomes,
            source_dir,
            optimized_dir,
            preserved,
            preserve_declined,
            pipeline_exits,
        ):
            report_f.write(f"{line}\n")
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
        savings_pct = savings / total_src_size * 100
        report_f.write("--- Storage Impact (Matched Files) ---\n")
        report_f.write(f"  Source total:    {format_size(total_src_size)}\n")
        report_f.write(f"  Optimized total: {format_size(total_opt_size)}\n")
        report_f.write(
            f"  Space saved:     {format_size(savings)} ({savings_pct:.1f}%)\n\n"
        )

    handoff_count = len(pipeline_handoff)
    vid_failed_count = len(vid_pipeline_failed)
    vid_unverified_count = len(vid_pipeline_unverified)
    explained_gaps = (
        len(true_missing) + handoff_count + vid_failed_count + vid_unverified_count
    )
    expected_count_delta = -handoff_count
    count_matches_with_handoff = delta == 0 or (
        vid_failed_count == 0 and delta == expected_count_delta
    )
    count_fully_explained = (
        delta != 0 and abs(delta) == explained_gaps and explained_gaps > 0
    )
    missing_total = (
        len(true_missing) + handoff_count + vid_failed_count + vid_unverified_count
    )
    return {
        "source": str(source_dir),
        "optimized": str(optimized_dir),
        "scope": processing_mode,
        "source_files": src_count,
        "optimized_files": opt_count,
        "count_delta": delta,
        "count_matches_with_handoff": count_matches_with_handoff,
        "count_fully_explained": count_fully_explained,
        "explained_gaps": explained_gaps,
        "expected_count_delta": expected_count_delta,
        "matched": len(matched),
        "ambiguous": len(ambiguous),
        "missing": len(true_missing),
        "pipeline_handoff": handoff_count,
        "vid_pipeline_failed": vid_failed_count,
        "vid_pipeline_unverified": vid_unverified_count,
        "missing_total": missing_total,
        "extra": len(extra),
        "mismatched_types": len(mismatched_types),
        "source_total_size": total_src_size,
        "optimized_total_size": total_opt_size,
        "integrity_failures": len(ambiguous)
        + len(true_missing)
        + vid_failed_count
        + len(extra)
        + len(mismatched_types),
        "has_warnings": bool(
            (not count_matches_with_handoff and not count_fully_explained)
            or ambiguous
            or true_missing
            or vid_pipeline_failed
            or vid_pipeline_unverified
            or mismatched_types
        ),
    }


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
    uncertain_pattern = re.compile(
        r"Tree uncertain \(([^)]+)\) \[prob=([\d.]+)\].*falling back to Layer 6 KNN"
    )
    knn_bypass_pattern = re.compile(
        r"Loop DB unavailable or disabled — running tree without KNN"
    )
    tree_uncertain_pattern = re.compile(
        r"Tree-only result remained uncertain \(([^)]+)\)"
    )

    modern_true_formats = {"webp", "avif", "jxl", "heic", "heif"}
    target_formats = {"GIF", "MOV", "MP4", "HEVC", "AV1"}

    def source_has_modern_true_format(source: str) -> bool:
        return detect_true_format(Path(source)) in modern_true_formats

    results = []
    uncertain_cases = []
    source_probe_errors = []
    log_dir_path = unified_log_dir()

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
                        if m := checking_pattern.search(line):
                            current_file = m.group(1).strip()

                        # Check if current_file is within filter_dir
                        is_relevant = True
                        if filter_dir_abs and current_file:
                            try:
                                abs_current = str(Path(current_file).resolve())
                                if not abs_current.startswith(filter_dir_abs):
                                    is_relevant = False
                            except (
                                OSError,
                                ValueError,
                                RuntimeError,
                                TypeError,
                                KeyError,
                                IndexError,
                                AttributeError,
                                UnicodeError,
                            ):
                                pass

                        if not is_relevant:
                            continue

                        # Conversions
                        if m := result_pattern.search(line):
                            source = m.group(1).split(">")[-1].strip()
                            current_file = source

                            # Re-verify relevance for conversion line
                            if filter_dir_abs:
                                try:
                                    if not str(Path(source).resolve()).startswith(
                                        filter_dir_abs
                                    ):
                                        continue
                                except (
                                    OSError,
                                    ValueError,
                                    RuntimeError,
                                    TypeError,
                                    KeyError,
                                    IndexError,
                                    AttributeError,
                                    UnicodeError,
                                ):
                                    pass

                            target, msg, status_icon = (
                                m.group(2).strip(),
                                m.group(3).strip(),
                                m.group(4),
                            )
                            try:
                                source_is_modern = source_has_modern_true_format(source)
                            except OSError as exc:
                                source_probe_errors.append(
                                    (source, log_file.name, str(exc))
                                )
                                source_is_modern = False
                            if source_is_modern and any(
                                f in target.upper() or f in msg.upper()
                                for f in target_formats
                            ):
                                results.append(
                                    {
                                        "log": log_file.name,
                                        "source": source,
                                        "target": target,
                                        "status": "SUCCESS"
                                        if status_icon == "✅"
                                        else "FAILED",
                                        "details": msg,
                                    }
                                )

                        if m := activity_pattern.search(line):
                            target_fmt, details, source = (
                                m.group(1).strip(),
                                m.group(2).strip(),
                                m.group(3).strip(),
                            )
                            current_file = source

                            # Re-verify relevance
                            if filter_dir_abs:
                                try:
                                    if not str(Path(source).resolve()).startswith(
                                        filter_dir_abs
                                    ):
                                        continue
                                except (
                                    OSError,
                                    ValueError,
                                    RuntimeError,
                                    TypeError,
                                    KeyError,
                                    IndexError,
                                    AttributeError,
                                    UnicodeError,
                                ):
                                    pass

                            try:
                                source_is_modern = source_has_modern_true_format(source)
                            except OSError as exc:
                                source_probe_errors.append(
                                    (source, log_file.name, str(exc))
                                )
                                source_is_modern = False
                            if source_is_modern and any(
                                f in target_fmt.upper() for f in target_formats
                            ):
                                results.append(
                                    {
                                        "log": log_file.name,
                                        "source": source,
                                        "target": f"CONVERTED TO {target_fmt}",
                                        "status": "PROCESSING/UNKNOWN",
                                        "details": details,
                                    }
                                )

                        # Loop Intent
                        if (
                            uncertain_pattern.search(line)
                            or knn_bypass_pattern.search(line)
                            or tree_uncertain_pattern.search(line)
                        ):
                            if current_file:
                                reason, prob = "N/A", "N/A"
                                if u := uncertain_pattern.search(line):
                                    reason, prob = u.group(1), u.group(2)
                                elif knn_bypass_pattern.search(line):
                                    reason = "KNN Bypassed (DB Unavailable)"
                                elif t := tree_uncertain_pattern.search(line):
                                    reason = t.group(1)

                                # Duplicate check
                                if not any(
                                    c["file"] == current_file
                                    and c["log"] == log_file.name
                                    for c in uncertain_cases
                                ):
                                    matching_folders = []
                                    if log_dir_path.exists():
                                        stem = Path(current_file).stem
                                        for item in log_dir_path.iterdir():
                                            if item.is_dir() and stem in item.name:
                                                matching_folders.append(item.name)

                                    uncertain_cases.append(
                                        {
                                            "file": current_file,
                                            "reason": reason,
                                            "probability": prob,
                                            "log": log_file.name,
                                            "matching_folders": matching_folders,
                                        }
                                    )

            except OSError as e:
                raise RuntimeError(f"log file unreadable: {log_file}: {e}") from e

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

    unique_probe_errors = []
    seen_probe_errors = set()
    for source, log_name, detail in source_probe_errors:
        key = (source, log_name, detail)
        if key not in seen_probe_errors:
            unique_probe_errors.append((source, log_name, detail))
            seen_probe_errors.add(key)

    report_f.write("── LOOP INTENT EDGE CASES (UNCERTAIN / KNN BYPASSED) ──────────\n")
    if not unique_uncertain:
        report_f.write("No uncertain loop intent cases found.\n\n")
    else:
        for i, c in enumerate(unique_uncertain, 1):
            report_f.write(
                f"[{i}] FILE: {c['file']}\n    REASON: {c['reason']}\n    PROB:   {c['probability']}\n    LOG:    {c['log']}\n"
            )
            if c["matching_folders"]:
                report_f.write(f"    FOLDERS: {', '.join(c['matching_folders'])}\n")
            report_f.write("-" * 40 + "\n")
        report_f.write("\n")

    report_f.write("── MODERN TO LEGACY CONVERSIONS ───────────────────────────────\n")
    if not unique_results:
        report_f.write("No conversions found.\n")
    else:
        for i, r in enumerate(unique_results, 1):
            report_f.write(
                f"[{i}] SOURCE: {r['source']}\n    TARGET: {r['target']}\n    STATUS: {r['status']}\n    INFO:   {r['details']}\n    LOG:    {r['log']}\n"
            )
            report_f.write("-" * 40 + "\n")

    if unique_probe_errors:
        report_f.write("\n")
        report_f.write(
            "── LOG SOURCE FORMAT PROBE ERRORS ─────────────────────────────\n"
        )
        for i, (source, log_name, detail) in enumerate(unique_probe_errors, 1):
            report_f.write(
                f"[{i}] SOURCE: {source}\n    LOG:    {log_name}\n    ERROR:  {detail}\n"
            )
            report_f.write("-" * 40 + "\n")

    return len(unique_results), len(unique_uncertain)


# ---------------------------------------------------------------------------
# Main Execution
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import sys
    from pathlib import Path

    _scripts = Path(__file__).resolve().parent
    if str(_scripts) not in sys.path:
        sys.path.insert(0, str(_scripts))
    from mfb_entry_guard import guard_main  # noqa: E402

    guard_main("verify.py")
    parser = argparse.ArgumentParser(
        description="MFB Conversion Analyzer & Integrity Verifier"
    )
    parser.add_argument("logs", nargs="*", help="Log files or directories to scan.")
    parser.add_argument(
        "--verify",
        nargs="+",
        help="Source and/or optimized directories for integrity check (auto-detects if one provided).",
    )
    parser.add_argument("-o", "--output", help="Custom output report path.")
    parser.add_argument(
        "--print-integrity-summary",
        action="store_true",
        help="Print integrity summary to stdout for automation pipelines.",
    )
    parser.add_argument(
        "--fast-img-delivery",
        action="store_true",
        help="Verify fast-img post-delivery invariant: source true JPEGs deleted and optimized output is JXL-only.",
    )
    parser.add_argument(
        "--fast-img-restore",
        action="store_true",
        help="Verify Fastmode Img restore-jpeg invariant: source true JXLs decode back to restored true JPEGs.",
    )
    parser.add_argument(
        "--mode",
        choices=("both", "images_only", "videos_only"),
        default="both",
        help="Limit integrity verification to the same processing scope used by the runtime.",
    )
    parser.add_argument(
        "--session-audit",
        action="append",
        default=[],
        metavar="PATH",
        help="Session verbose log(s) with ROUTED pipeline= lines (from drag-and-drop). Repeatable.",
    )

    args = parser.parse_args()

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    output_report = Path(
        args.output
        if args.output
        else unified_log_dir() / f"diagnostic_report_{timestamp}.txt"
    )
    output_report.parent.mkdir(parents=True, exist_ok=True)

    with open(output_report, "w", encoding="utf-8") as report_f:
        report_f.write(
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"
        )
        report_f.write("      MODERN FORMAT BOOST - DIAGNOSTIC ANALYSIS REPORT\n")
        report_f.write(
            f"      Generated at: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}\n"
        )
        report_f.write(
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n"
        )

        # 1. Integrity Check
        source_dir_context = None
        integrity_stats = None
        session_audit_paths = [Path(p).resolve() for p in args.session_audit]
        bundle_log_dir = unified_log_dir()
        explicit_log_paths = None
        if args.logs:
            files = [Path(p).resolve() for p in args.logs if Path(p).is_file()]
            if files:
                explicit_log_paths = files
                bundle_log_dir = None
            else:
                first_log = Path(args.logs[0]).resolve()
                if first_log.is_dir():
                    bundle_log_dir = first_log

        if args.verify:
            resolved = resolve_verify_dirs(args.verify)
            if resolved:
                src, opt = resolved
                source_dir_context = src
                if args.fast_img_delivery:
                    integrity_stats = run_fast_img_delivery_check(
                        src,
                        opt,
                        report_f,
                        args.mode,
                    )
                elif args.fast_img_restore:
                    integrity_stats = run_fast_img_restore_check(
                        src,
                        opt,
                        report_f,
                    )
                else:
                    integrity_stats = run_integrity_check(
                        src,
                        opt,
                        report_f,
                        args.mode,
                        session_audit_paths=session_audit_paths or None,
                        bundle_log_dir=bundle_log_dir,
                        explicit_log_paths=explicit_log_paths,
                    )
            else:
                report_f.write(
                    f"{pick_symbol('❌', ('[ERROR]'))} Error: Could not resolve paired directory for {args.verify[0]}\n\n"
                )

        # 2. Log Analysis
        log_inputs = [str(p) for p in args.logs]
        if not log_inputs and not args.fast_img_delivery and not args.fast_img_restore:
            log_inputs = [str(bundle_log_dir)]
        if log_inputs:
            conv_count, unc_count = parse_logs(
                log_inputs, report_f, filter_dir=source_dir_context
            )
            print(
                f"{pick_symbol('📈', ('[CHART]'))} Total conversion events: {conv_count}"
            )
            print(f"🔭 Uncertain loop cases: {unc_count}")

    if args.print_integrity_summary:
        if integrity_stats is None:
            print(
                "🔎 Integrity summary: unavailable (source/optimized pair not resolved)"
            )
        else:
            delta = integrity_stats["count_delta"]
            if integrity_stats.get("count_status_label"):
                delta_text = integrity_stats["count_status_label"]
            elif integrity_stats.get("integrity_failures", 0) > 0:
                issue_count = integrity_stats.get("integrity_failures", 0)
                delta_text = (
                    f"MISMATCH ({issue_count} "
                    f"{integrity_stats.get('integrity_kind', 'integrity')} invariant "
                    f"issue{'' if issue_count == 1 else 's'})"
                )
            elif integrity_stats.get("count_matches_with_handoff"):
                handoff_n = integrity_stats.get("pipeline_handoff", 0)
                if handoff_n > 0 and delta == integrity_stats.get(
                    "expected_count_delta", 0
                ):
                    delta_text = (
                        f"MATCH ({handoff_n} expected handoff gap"
                        f"{'' if handoff_n == 1 else 's'})"
                    )
                else:
                    delta_text = "MATCH"
            elif integrity_stats.get("count_fully_explained"):
                direction = "more" if delta > 0 else "fewer"
                explained = integrity_stats.get("explained_gaps", 0)
                delta_text = f"EXPLAINED ({abs(delta)} {direction}; all {explained} listed below)"
            else:
                direction = "more" if delta > 0 else "fewer"
                explained = integrity_stats.get("explained_gaps", 0)
                unexplained = abs(abs(delta) - explained)
                if unexplained:
                    delta_text = (
                        f"MISMATCH ({abs(delta)} {direction}; "
                        f"{unexplained} still unexplained)"
                    )
                else:
                    delta_text = f"MISMATCH ({abs(delta)} {direction} in optimized)"
            print(f"{pick_symbol('🔎', ('[CHECK]'))} Integrity summary")
            print(f"   Source:    {integrity_stats['source']}")
            optimized_path_label = integrity_stats.get(
                "optimized_path_label", "Optimized"
            )
            print(f"   {optimized_path_label}: {integrity_stats['optimized']}")
            print(f"   Scope:           {integrity_stats['scope']}")
            source_label = integrity_stats.get("source_files_label", "Source files")
            optimized_label = integrity_stats.get(
                "optimized_files_label", "Optimized files"
            )
            print(f"   {source_label + ':':<34}{integrity_stats['source_files']}")
            print(f"   {optimized_label + ':':<34}{integrity_stats['optimized_files']}")
            if integrity_stats.get("tier2_recorded", 0) > 0:
                print(
                    f"   {'Recorded tier-2 modern lossy:':<34}{integrity_stats['tier2_recorded']}"
                )
                print(
                    f"   {'Verified tier-2 deleted:':<34}{integrity_stats['tier2_verified_deleted']}"
                )
            if "skipped_sources" in integrity_stats:
                print(
                    f"   {'Recorded skipped JPEGs:':<34}{integrity_stats['skipped_sources']}"
                )
            if "failed_sources" in integrity_stats:
                print(
                    f"   {'Recorded failed JPEGs:':<34}{integrity_stats['failed_sources']}"
                )
            if "source_remaining_files" in integrity_stats:
                remaining_label = integrity_stats.get(
                    "source_remaining_label", "Source files remaining"
                )
                print(
                    f"   {remaining_label + ':':<34}{integrity_stats['source_remaining_files']}"
                )
            print(f"   Count status:    {delta_text}")
            print(f"   Matched:         {integrity_stats['matched']}")
            print(f"   Ambiguous:       {integrity_stats['ambiguous']}")
            print(
                f"   Missing:         {integrity_stats['missing']} (static / data-loss risk)"
            )
            print(
                f"   Handoff gaps:    {integrity_stats.get('pipeline_handoff', 0)} "
                f"(expected gap — scope / vid static ignore)"
            )
            print(
                f"   Vid failures:    {integrity_stats.get('vid_pipeline_failed', 0)} "
                f"(vid failed/skipped when transcode required)"
            )
            print(
                f"   Vid unverified:  {integrity_stats.get('vid_pipeline_unverified', 0)} "
                f"(attach session/bundle logs for mfb::audit)"
            )
            print(f"   Extra:           {integrity_stats['extra']}")
            print(f"   Type mismatch:   {integrity_stats['mismatched_types']}")
            print(f"   Integrity Issues:{integrity_stats['integrity_failures']}")
            print(
                f"   Integrity:      {'WARNINGS' if integrity_stats['has_warnings'] else 'CLEAN'}"
            )
            if integrity_stats["source_total_size"] > 0:
                src_size = integrity_stats["source_total_size"]
                opt_size = integrity_stats["optimized_total_size"]
                savings = src_size - opt_size
                savings_pct = (savings / src_size) * 100
                print(
                    f"   Space saved:     {format_size(savings)} ({savings_pct:.1f}%)"
                )

    print(f"{pick_symbol('📊', ('[STATS]'))} Full report generated: {output_report}")

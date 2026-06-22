"""Fast image mode path policy.

Fastmode is a JPEG-only Rust pipeline. Its local delivery directory is always a
source-adjacent ``*_optimized`` folder, never a copy of the source tree.
"""

from __future__ import annotations

import os
from collections.abc import Callable
from pathlib import Path


MFB_DEFAULT_HOME_DIRNAME = ".modern_format_boost"
FAST_IMG_FORCE_SMART_BUILD = False


def default_mfb_state_root() -> Path:
    """Return the persistent user state root used by app and terminal launches."""
    env_root = (os.environ.get("MFB_HOME_ROOT") or "").strip()
    if env_root:
        return Path(env_root).expanduser()
    if home := os.environ.get("HOME"):
        return Path(home).expanduser() / MFB_DEFAULT_HOME_DIRNAME
    return Path.home() / MFB_DEFAULT_HOME_DIRNAME


def fast_img_output_dir_for_target(
    target_dir: Path,
    has_resume_marker: Callable[[Path], bool] | None = None,
) -> Path:
    """Resolve fastmode's adjacent JXL-only output directory.

    Mirrors Rust ``resolve_working_copy_dir`` collision policy: an existing
    directory is reused only when it carries a resume marker — the legacy
    in-tree ``.mfb_wc`` or (via ``has_resume_marker``) the central state-dir
    marker the current Rust binary writes. Without the callback Python would
    skip to ``_optimized_2`` while Rust resumes into ``_optimized``.
    """
    target = target_dir.expanduser()
    base = target.with_name(f"{target.name}_optimized")
    candidate = base
    suffix = 2
    # Legacy Rust builds wrote `.mfb_wc` inside the output tree. Treat it only as
    # a one-time resume signal so the fixed Rust binary can migrate/delete it.
    while (
        candidate.exists()
        and not (candidate / ".mfb_wc").exists()
        and not (has_resume_marker(candidate) if has_resume_marker else False)
    ):
        candidate = base.with_name(f"{base.name}_{suffix}")
        suffix += 1
    return candidate


def _unique_adjacent_dir(target_dir: Path, suffix_name: str) -> Path:
    target = target_dir.expanduser()
    base = target.with_name(f"{target.name}_{suffix_name}")
    candidate = base
    suffix = 2
    while candidate.exists():
        candidate = base.with_name(f"{base.name}_{suffix}")
        suffix += 1
    return candidate


def fast_img_restore_output_dir_for_target(target_dir: Path) -> Path:
    """Resolve the adjacent JPEG restoration output directory."""
    return _unique_adjacent_dir(target_dir, "restored_jpeg")


def fast_vid_output_dir_for_target(target_dir: Path) -> Path:
    """Resolve the adjacent full-pipeline output directory for vid FastMode."""
    return _unique_adjacent_dir(target_dir, "optimized")


def build_fast_img_command(
    img_binary: Path,
    target_dir: Path,
    *,
    shortest_path: bool,
    archive: bool,
    retry: bool = False,
) -> list[str]:
    """Build the Rust fast-img command for drag-and-drop launches."""
    command = [
        str(img_binary),
        "fast-img",
        str(target_dir),
        "--recursive",
    ]
    if retry:
        command.append("--retry")
    if archive:
        command.append("--archive")
    if shortest_path:
        command.extend(["--shortest-path", "--auto-import"])
    return command


def build_fast_img_restore_command(
    img_binary: Path, target_dir: Path, *, output_dir: Path
) -> list[str]:
    """Build the Rust JXL-to-JPEG restore command for drag-and-drop launches."""
    return [
        str(img_binary),
        "restore-jpeg",
        str(target_dir),
        "--output",
        str(output_dir),
        "--recursive",
    ]


def build_fast_vid_command(
    vid_binary: Path,
    target_dir: Path,
    *,
    output_dir: Path,
    shortest_path: bool,
) -> list[str]:
    """Build the Rust full loop-intent video FastMode command."""
    del shortest_path
    command = [
        str(vid_binary),
        "run",
        str(target_dir),
        "--output",
        str(output_dir),
        "--base-dir",
        str(target_dir),
        "--recursive",
        "--apple-compat",
        "--ultimate",
        "--archive",
    ]
    return command

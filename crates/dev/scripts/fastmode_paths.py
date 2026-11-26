"""Fast image mode path policy.

Fastmode is a JPEG-only Rust pipeline. Its local delivery directory is always a
source-adjacent ``*_optimized`` folder, never a copy of the source tree.
"""

from __future__ import annotations

import os
import tempfile
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
    return Path(tempfile.gettempdir()) / MFB_DEFAULT_HOME_DIRNAME


def _adjacent_dir(target_dir: Path, suffix_name: str) -> Path:
    target = target_dir.expanduser()
    return target.with_name(f"{target.name}_{suffix_name}")


def fast_img_restore_output_dir_for_target(target_dir: Path) -> Path:
    """Resolve the stable adjacent JPEG restoration output directory."""
    return _adjacent_dir(target_dir, "restored_jpeg")


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
        command.append("--shortest-path")
    return command


def build_fast_img_restore_command(
    img_binary: Path,
    target_dir: Path,
    *,
    output_dir: Path | None = None,
    photos_album_id: str | None = None,
    photos_folder_id: str | None = None,
) -> list[str]:
    """Build the Rust JXL-to-JPEG restore command for drag-and-drop launches."""
    if photos_album_id and photos_folder_id:
        raise ValueError("select either a Photos album or folder UUID")
    command = [
        str(img_binary),
        "restore-jpeg",
        str(target_dir),
        "--recursive",
    ]
    if output_dir is not None:
        command.extend(["--output", str(output_dir)])
    if photos_album_id:
        command.extend(["--photos-album-id", photos_album_id])
    if photos_folder_id:
        command.extend(["--photos-folder-id", photos_folder_id])
    return command


def build_fast_vid_command(
    vid_binary: Path, target: Path, *, shortest_path: bool = False
) -> list[str]:
    """Check GIF originals locally or deliver them to Photos when selected."""
    command = [str(vid_binary), "fast-vid"]
    if shortest_path:
        command.append("--shortest-path")
    return [*command, "--", str(target)]

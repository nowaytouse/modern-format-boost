"""Stable ``foundation`` dylib for training lanes (``python_api`` ctypes).

Copies ``target/release`` → ``.modern_format_boost/artifacts/`` when missing or
stale so lane workers never keep running an old build after ``cargo rustc -p
foundation``.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

from fastmode_paths import default_mfb_state_root

SCRIPT_DIR = Path(__file__).resolve().parent
ROOT = SCRIPT_DIR.parents[2]
HOME_ROOT = default_mfb_state_root()
ARTIFACT_DIR = HOME_ROOT / "artifacts"


def rust_dylib_filename() -> str:
    if sys.platform == "darwin":
        return "libfoundation.dylib"
    if sys.platform == "win32":
        return "foundation.dll"
    return "libfoundation.so"


def target_release_dylib() -> Path:
    return ROOT / "target" / "release" / rust_dylib_filename()


def artifact_dylib() -> Path:
    return ARTIFACT_DIR / rust_dylib_filename()


def _artifact_stale(artifact: Path, built: Path) -> bool:
    if not artifact.is_file() or not built.is_file():
        return True
    return built.stat().st_mtime > artifact.stat().st_mtime


def app_bundle_dylib_path() -> Path | None:
    lib_name = rust_dylib_filename()
    exe_path = Path(sys.executable).resolve()
    for ancestor in [exe_path] + list(exe_path.parents):
        if ancestor.suffix == ".app":
            fw = ancestor / "Contents" / "Frameworks" / lib_name
            res = ancestor / "Contents" / "Resources" / lib_name
            if fw.is_file():
                return fw
            if res.is_file():
                return res

    app_bundle = ROOT / "Modern Format Boost.app"
    if app_bundle.is_dir():
        fw = app_bundle / "Contents" / "Frameworks" / lib_name
        res = app_bundle / "Contents" / "Resources" / lib_name
        if fw.is_file():
            return fw
        if res.is_file():
            return res

    return None


def ensure_foundation_dylib(*, force_rebuild: bool = False) -> str:
    """Return path to stable dylib; rebuild/copy when missing or stale."""
    artifact = artifact_dylib()
    built = target_release_dylib()
    if not force_rebuild:
        app_dylib = app_bundle_dylib_path()
        if app_dylib is not None and not any(
            path.is_file() and path.stat().st_mtime > app_dylib.stat().st_mtime
            for path in (artifact, built)
        ):
            return str(app_dylib)

    ARTIFACT_DIR.mkdir(parents=True, exist_ok=True)

    if force_rebuild or not built.is_file():
        print(
            "  [BUILD] syncing foundation dylib"
            + (" (forced)" if force_rebuild else " (release dylib missing)"),
            flush=True,
        )
        subprocess.run(
            [
                "cargo",
                "rustc",
                "--release",
                "-p",
                "foundation",
                "--lib",
                "--crate-type",
                "cdylib",
            ],
            cwd=str(ROOT),
            check=True,
        )

    if not built.is_file():
        raise RuntimeError(f"cargo build succeeded but dylib still missing: {built}")

    if force_rebuild or not artifact.is_file() or _artifact_stale(artifact, built):
        shutil.copy2(str(built), str(artifact))
        print(f"  [DYLIB] synced {artifact}", flush=True)

    return str(artifact)


def apply_foundation_lib_env(*, force_rebuild: bool = False) -> str:
    """Set ``SHARED_UTILS_LIB_PATH`` for child lane workers if unset."""
    path = ensure_foundation_dylib(force_rebuild=force_rebuild)
    os.environ.setdefault("SHARED_UTILS_LIB_PATH", path)
    return path

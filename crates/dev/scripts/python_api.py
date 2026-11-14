from __future__ import annotations

import ctypes
import json
import os
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Final

from fastmode_paths import default_mfb_state_root


def discover_root(script_path: Path) -> Path:
    expected_relative = Path("crates") / "dev" / "scripts" / script_path.name
    for candidate in script_path.parents:
        if (candidate / "Cargo.toml").exists() and (
            candidate / expected_relative
        ).exists():
            return candidate
    raise SystemExit(
        f"Could not locate repository root from {script_path}; "
        f"expected Cargo.toml and {expected_relative.as_posix()}"
    )


# Locate the compiled Rust dynamic library
ROOT = discover_root(Path(__file__).resolve())
LIB_DIR = ROOT / "target" / "debug"
RELEASE_LIB_DIR = ROOT / "target" / "release"
DEFAULT_CONNSTR: Final = "postgresql://localhost/modern_format_boost"

# Rust C-API batch abort codes (pre-path); surfaced via get_last_ingest_error().
INGEST_BATCH_FATAL_CODES: Final[frozenset[int]] = frozenset({-1, -2, -3, -4, -5})

rust_lib: ctypes.CDLL | None = None


def reset_rust_lib_cache() -> None:
    """Drop cached CDLL so the next call reloads ``SHARED_UTILS_LIB_PATH``."""
    global rust_lib
    rust_lib = None


def candidate_library_paths() -> list[Path]:
    override = os.environ.get("SHARED_UTILS_LIB_PATH")
    if override:
        return [Path(override)]

    if sys.platform == "darwin":
        lib_name = "libfoundation.dylib"
    elif sys.platform == "win32":
        lib_name = "foundation.dll"
    else:
        lib_name = "libfoundation.so"

    artifact = default_mfb_state_root() / "artifacts" / lib_name
    built = LIB_DIR / lib_name
    release = RELEASE_LIB_DIR / lib_name
    candidates = []
    scripts_dir = Path(__file__).resolve().parent
    if str(scripts_dir) not in sys.path:
        sys.path.insert(0, str(scripts_dir))
    from mfb_dylib import app_bundle_dylib_path

    app_dylib = app_bundle_dylib_path()
    if app_dylib is not None and not any(
        path.is_file() and path.stat().st_mtime > app_dylib.stat().st_mtime
        for path in (artifact, built, release)
    ):
        candidates.append(app_dylib)
    for p in [artifact, built, release]:
        if p not in candidates:
            candidates.append(p)

    return candidates


def _resolve_lib_path_for_load() -> Path:
    override = os.environ.get("SHARED_UTILS_LIB_PATH")
    if override:
        return Path(override)
    scripts_dir = Path(__file__).resolve().parent
    if str(scripts_dir) not in sys.path:
        sys.path.insert(0, str(scripts_dir))
    from mfb_dylib import ensure_foundation_dylib

    return Path(ensure_foundation_dylib())


def resolved_library_path() -> Path:
    candidates = candidate_library_paths()
    return next((p for p in candidates if p.exists()), candidates[0])


def _load_rust_lib() -> ctypes.CDLL:
    global rust_lib
    if rust_lib is not None:
        return rust_lib

    lib_path = _resolve_lib_path_for_load()
    if not lib_path.is_file():
        raise FileNotFoundError(
            f"Could not find Rust library at {lib_path}. "
            "Run `cargo rustc -p foundation --lib --crate-type cdylib` first or disable --use-api."
        )

    rust_lib = ctypes.CDLL(str(lib_path))
    rust_lib.ingest_media_samples_batch.argtypes = [
        ctypes.c_char_p,  # conn_str_ptr
        ctypes.c_char_p,  # paths_ptr (JSON array or pipe-separated fallback)
        ctypes.c_char_p,  # label_ptr
        ctypes.c_char_p,  # scenario_ptr
    ]
    rust_lib.ingest_media_samples_batch.restype = ctypes.c_int
    rust_lib.mfb_last_ingest_error.argtypes = []
    rust_lib.mfb_last_ingest_error.restype = ctypes.c_char_p
    rust_lib.mfb_probe_static_still_image.argtypes = [ctypes.c_char_p]
    rust_lib.mfb_probe_static_still_image.restype = ctypes.c_void_p
    rust_lib.mfb_probe_loop_intent.argtypes = [ctypes.c_char_p]
    rust_lib.mfb_probe_loop_intent.restype = ctypes.c_void_p
    rust_lib.mfb_free_string.argtypes = [ctypes.c_void_p]
    rust_lib.mfb_free_string.restype = None
    return rust_lib


def get_last_ingest_error() -> str:
    """
    Return the last C-API ingestion diagnostic string.

    Rust stores this after each path attempted inside `ingest_media_samples_batch`
    (notably when the per-path count is zero, the path is missing, or the DB layer
    bails with a label/score conflict).

    The raw pointer returned by `mfb_last_ingest_error` in the dylib is only valid
    until the next ingest call; this helper copies the string immediately.
    """
    lib = _load_rust_lib()
    raw = lib.mfb_last_ingest_error()
    if raw is None:
        return ""
    if isinstance(raw, bytes):
        return raw.decode("utf-8", errors="replace")
    return str(raw)


def ingest_media_samples_batch(
    conn_str: str, file_paths: Sequence[str], label: str | None, scenario: str
) -> int:
    """
    Ingest a batch of media samples into the database using a single connection.

    Args:
        conn_str: PostgreSQL connection string.
        file_paths: List of absolute paths to the assets.
        label: The quality label. For `loop_intent`, pass `None`/`""` for auto-labeling.
        scenario: The target scenario.

    Returns:
        The number of successfully ingested files in this call, or a negative
        error code for connection/parse failures before any path runs.

        When the return value is less than ``len(file_paths)`` but non-negative,
        only some paths were ingested; call :func:`get_last_ingest_error` after
        the call (optionally with single-path batches) to recover per-path
        diagnostics. Training uses this to separate label conflicts from other
        failures when ``--use-api`` is enabled.
    """
    if not conn_str or not conn_str.strip():
        raise ValueError("conn_str must be a non-empty PostgreSQL connection string")
    if isinstance(file_paths, (str, bytes)):
        raise TypeError(
            "file_paths must be a sequence of non-empty paths, not a single string"
        )
    if len(file_paths) == 0:
        return 0
    if not scenario or not scenario.strip():
        raise ValueError("scenario must be a non-empty target scenario")
    if scenario != "loop_intent" and (not label or not label.strip()):
        raise ValueError("label must be a non-empty quality label")

    normalized_paths: list[str] = []
    for path in file_paths:
        normalized_path = path.strip()
        if not normalized_path:
            raise ValueError("file_paths must contain only non-empty strings")
        normalized_paths.append(normalized_path)

    lib = _load_rust_lib()
    conn_str_c = conn_str.encode("utf-8")
    paths_c = json.dumps(normalized_paths).encode("utf-8")
    label_c = (label or "").encode("utf-8")
    scenario_c = scenario.encode("utf-8")

    return lib.ingest_media_samples_batch(conn_str_c, paths_c, label_c, scenario_c)


def _probe_json_from_c_api(c_fn, file_path: str) -> dict:
    normalized_path = file_path.strip()
    if not normalized_path:
        raise ValueError("file_path must be a non-empty string")
    lib = _load_rust_lib()
    raw_ptr = c_fn(normalized_path.encode("utf-8"))
    if not raw_ptr:
        return {"ok": False, "error": "probe returned null"}
    try:
        raw = ctypes.cast(raw_ptr, ctypes.c_char_p).value
        payload = (raw or b"").decode("utf-8", errors="replace")
    finally:
        lib.mfb_free_string(raw_ptr)
    try:
        parsed = json.loads(payload)
    except json.JSONDecodeError as exc:
        return {"ok": False, "error": f"invalid probe JSON: {exc}"}
    if not isinstance(parsed, dict):
        return {"ok": False, "error": "probe JSON must be an object"}
    return parsed


def probe_static_still_image(file_path: str) -> dict:
    """
    Probe static still tier signals via Rust ``analyze_image`` (same as DB ingest).

    Returns a dict with ``ok: true`` and width/height/entropy/tier fields, or
    ``ok: false`` and ``error``.
    """
    lib = _load_rust_lib()
    return _probe_json_from_c_api(lib.mfb_probe_static_still_image, file_path)


def probe_loop_intent(file_path: str) -> dict:
    """Probe loop vs non-loop bucket via Rust ``sample_from_path`` heuristics."""
    lib = _load_rust_lib()
    return _probe_json_from_c_api(lib.mfb_probe_loop_intent, file_path)


def ingest_media_sample(
    conn_str: str, file_path: str, label: str | None, scenario: str
) -> int:
    """
    Directly ingest a media sample into the database using the Rust Multi-Scenario API.

    Args:
        conn_str: PostgreSQL connection string (e.g., from local_env.sh).
        file_path: Absolute path to the image, GIF, or video.
        label: The quality or intent label (normally "high" or "low" for
            quality scenarios; image-format family is resolved by Rust).
        scenario: The target multi-scenario embedding table ("image_quality", "animated_image_quality", "video_quality", "loop_intent").

    Returns:
        ``1`` if that file was ingested, ``0`` if it was skipped or not ingested
        for a per-path reason, or a negative code for early failures (connection,
        schema, parse). On non-success, see :func:`get_last_ingest_error`.
    """
    normalized_path = file_path.strip()
    if not normalized_path:
        raise ValueError("file_path must be a non-empty string")
    return ingest_media_samples_batch(conn_str, [normalized_path], label, scenario)


if __name__ == "__main__":
    import sys
    from pathlib import Path

    _scripts = Path(__file__).resolve().parent
    if str(_scripts) not in sys.path:
        sys.path.insert(0, str(_scripts))
    from mfb_entry_guard import guard_main

    guard_main("python_api.py")
    print("Modern Format Boost - Python to Rust Direct Ingestion API")
    print(f"Loaded library from: {resolved_library_path()}")
    _load_rust_lib()

    # This is an example of how to use the API directly without subprocess overhead.
    conn = (
        os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
    ).strip() or DEFAULT_CONNSTR

    # Deterministic demo: missing path sets the last-ingest diagnostic (no successful insert).
    demo_missing = ROOT / "__mfb_python_api_demo_missing_path__"
    ret_missing = ingest_media_sample(conn, str(demo_missing), "high", "image_quality")
    diag_missing = get_last_ingest_error()
    print(
        f"Demo (missing file): ingest_media_sample -> {ret_missing}, "
        f"get_last_ingest_error -> {diag_missing!r}"
    )

    test_file = (
        ROOT / "crates/dev/tests/edge/gifs/test.gif"
    )  # Replace with actual test file path if needed

    if test_file.exists():
        result = ingest_media_sample(
            conn, str(test_file), "high", "animated_image_quality"
        )
        if result > 0:
            print(
                "Successfully ingested sample into animated_image_quality via Rust C-API."
            )
        else:
            detail = get_last_ingest_error()
            print(f"Failed to ingest sample. Error code: {result}")
            if detail:
                print(f"Last ingest diagnostic: {detail}")
    else:
        print(
            "Optional test gif not found; missing-file demo above is enough to try the API."
        )

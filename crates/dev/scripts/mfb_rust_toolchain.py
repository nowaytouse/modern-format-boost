"""Resolve rustup toolchain binaries for scripts when shims need a PATH fallback.

**Root fix on Homebrew macOS:** ``~/.cargo/bin/rustup`` must point at the real
``libexec/bin/rustup`` binary, not Homebrew's shell wrapper (see
``repair_rustup_shims`` bin). Without that, toolchain ``cargo`` loads
``cargo-clippy`` from ``CARGO_HOME/bin`` and rustup mis-parses subcommands.

This module prepends ``toolchains/<name>/bin`` when proxies are still broken.
"""

from __future__ import annotations

import os
import platform
import subprocess
import sys
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path


@dataclass(frozen=True, slots=True)
class RustToolchain:
    """Resolved nightly (or stable) toolchain install."""

    cargo: Path
    bin_dir: Path
    name: str | None
    clippy: Path | None
    rustfmt: Path | None

    def env(self) -> dict[str, str]:
        return toolchain_env(self)


def toolchain_name_from_cargo_path(cargo_path: Path) -> str | None:
    parts = cargo_path.resolve().parts
    try:
        toolchains_idx = parts.index("toolchains")
    except ValueError:
        return None
    if toolchains_idx + 3 >= len(parts):
        return None
    if parts[toolchains_idx + 2] != "bin" or parts[toolchains_idx + 3] != "cargo":
        return None
    return parts[toolchains_idx + 1]


def _default_host_triple() -> str:
    machine = platform.machine().lower()
    arch = "aarch64" if machine in {"arm64", "aarch64"} else "x86_64"
    os_name = "apple-darwin" if sys.platform == "darwin" else "unknown-linux-gnu"
    return f"{arch}-{os_name}"


def _toolchain_globs(prefer: str) -> tuple[str, ...]:
    host = _default_host_triple()
    if prefer == "nightly":
        return (f"nightly-{host}", "nightly-*", f"stable-{host}", "stable-*")
    return (f"stable-{host}", "stable-*", f"nightly-{host}", "nightly-*")


def resolve_rust_toolchain(*, prefer: str = "nightly") -> RustToolchain:
    """Locate ``toolchains/<name>/bin/cargo`` and component binaries."""
    rustup_home = Path(os.environ.get("RUSTUP_HOME", Path.home() / ".rustup"))
    toolchains_root = rustup_home / "toolchains"

    explicit = os.environ.get("RUSTUP_TOOLCHAIN", "").strip()
    if explicit:
        candidate = toolchains_root / explicit / "bin" / "cargo"
        if candidate.is_file():
            return _toolchain_from_cargo(candidate)

    try:
        result = subprocess.run(
            ["rustup", "which", "cargo"],
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
        if result.returncode == 0:
            cargo_txt = result.stdout.strip()
            if cargo_txt:
                cargo_path = Path(cargo_txt)
                if cargo_path.is_file():
                    return _toolchain_from_cargo(cargo_path)
    except (OSError, subprocess.TimeoutExpired):
        pass

    if toolchains_root.is_dir():
        for pattern in _toolchain_globs(prefer):
            for toolchain_dir in sorted(toolchains_root.glob(pattern), reverse=True):
                candidate = toolchain_dir / "bin" / "cargo"
                if candidate.is_file():
                    return _toolchain_from_cargo(candidate)

    found = shutil_which("cargo")
    if found:
        return _toolchain_from_cargo(Path(found))

    fallback = Path("cargo")
    return RustToolchain(
        cargo=fallback,
        bin_dir=Path("."),
        name=None,
        clippy=None,
        rustfmt=None,
    )


def _toolchain_from_cargo(cargo_path: Path) -> RustToolchain:
    bin_dir = cargo_path.parent
    clippy = bin_dir / "cargo-clippy"
    rustfmt = bin_dir / "cargo-fmt"
    return RustToolchain(
        cargo=cargo_path,
        bin_dir=bin_dir,
        name=toolchain_name_from_cargo_path(cargo_path),
        clippy=clippy if clippy.is_file() else None,
        rustfmt=rustfmt if rustfmt.is_file() else None,
    )


def toolchain_env(toolchain: RustToolchain) -> dict[str, str]:
    """``PATH`` + ``RUSTUP_TOOLCHAIN`` overrides for subprocesses."""
    env: dict[str, str] = {}
    if toolchain.bin_dir != Path("."):
        env["PATH"] = str(toolchain.bin_dir) + os.pathsep + os.environ.get("PATH", "")
    if toolchain.name:
        env["RUSTUP_TOOLCHAIN"] = toolchain.name
    return env


def apply_toolchain_env(toolchain: RustToolchain | None = None) -> RustToolchain:
    """Merge :func:`toolchain_env` into ``os.environ`` (in-place)."""
    tc = toolchain or resolve_rust_toolchain()
    os.environ.update(tc.env())
    return tc


def cargo_argv(toolchain: RustToolchain, *args: str) -> list[str]:
    """Build a ``cargo`` argv using the resolved toolchain ``cargo`` binary."""
    return [str(toolchain.cargo), *args]


def cargo_component_argv(
    toolchain: RustToolchain, component: str, *args: str
) -> list[str]:
    """Invoke ``cargo-<component>`` from the toolchain (not rustup shims)."""
    path = toolchain.bin_dir / f"cargo-{component}"
    if path.is_file():
        return [str(path), *args]
    return cargo_argv(toolchain, component, *args)


@lru_cache(maxsize=None)
def _cached_toolchain() -> RustToolchain:
    return resolve_rust_toolchain()


def has_cargo_component(component: str) -> bool:
    """True when ``cargo-<component> --version`` succeeds in the toolchain bin dir."""
    tc = _cached_toolchain()
    path = tc.bin_dir / f"cargo-{component}"
    if not path.is_file():
        return False
    try:
        return (
            subprocess.run(
                [str(path), "--version"],
                capture_output=True,
                check=False,
                timeout=15,
            ).returncode
            == 0
        )
    except (OSError, subprocess.TimeoutExpired):
        return False


def shutil_which(cmd: str) -> str | None:
    from shutil import which

    return which(cmd)

#!/usr/bin/env python3
# Optional PATH helper when ~/.cargo/bin shims are broken.
# Root fix (run once): crates/dev/scripts/ci/repair_rustup_shims.py

import os
import sys
import subprocess
from pathlib import Path


def resolve_toolchain_bin():
    # Try using rustup to find cargo path
    try:
        cargo_path = (
            subprocess.check_output(
                ["rustup", "which", "cargo"], stderr=subprocess.DEVNULL
            )
            .decode()
            .strip()
        )
        if cargo_path and os.path.isfile(cargo_path):
            return str(Path(cargo_path).parent)
    except Exception:
        pass

    host = os.environ.get("MFB_RUST_HOST")
    if not host:
        try:
            # rustc -vV
            rustc_info = subprocess.check_output(
                ["rustc", "-vV"], stderr=subprocess.DEVNULL
            ).decode()
            for line in rustc_info.splitlines():
                if line.startswith("host:"):
                    host = line.split(":", 1)[1].strip()
                    break
        except Exception:
            pass

    if not host:
        import platform

        machine = platform.machine()
        if machine in ("arm64", "aarch64"):
            host = (
                "aarch64-apple-darwin"
                if sys.platform == "darwin"
                else "aarch64-unknown-linux-gnu"
            )
        elif machine == "x86_64":
            host = (
                "x86_64-apple-darwin"
                if sys.platform == "darwin"
                else "x86_64-unknown-linux-gnu"
            )
        else:
            host = f"{machine}-unknown-linux-gnu"

    rustup_home = os.environ.get("RUSTUP_HOME") or str(Path.home() / ".rustup")
    toolchain_name = os.environ.get("RUSTUP_TOOLCHAIN") or f"nightly-{host}"
    return str(Path(rustup_home) / "toolchains" / toolchain_name / "bin")


def setup_cargo_env():
    bin_dir = resolve_toolchain_bin()
    if bin_dir:
        os.environ["PATH"] = bin_dir + os.pathsep + os.environ.get("PATH", "")
        if "RUSTUP_TOOLCHAIN" not in os.environ:
            os.environ["RUSTUP_TOOLCHAIN"] = Path(bin_dir).parent.name


if __name__ == "__main__":
    setup_cargo_env()
    # Print export commands for shell sourcing if run directly
    print(f'export PATH="{os.environ["PATH"]}"')
    print(f'export RUSTUP_TOOLCHAIN="{os.environ["RUSTUP_TOOLCHAIN"]}"')

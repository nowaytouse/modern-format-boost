#!/usr/bin/env python3
# Repair ~/.cargo/bin rustup proxies on Homebrew macOS installs.

import os
import sys
import shutil
import subprocess
import glob
from pathlib import Path
from datetime import datetime

DRY_RUN = "--dry-run" in sys.argv

cargo_home = os.environ.get("CARGO_HOME") or str(Path.home() / ".cargo")
CARGO_BIN = Path(cargo_home) / "bin"
CARGO_BIN.mkdir(parents=True, exist_ok=True)


def resolve_rustup_real():
    try:
        prefix = (
            subprocess.check_output(
                ["brew", "--prefix", "rustup"], stderr=subprocess.DEVNULL
            )
            .decode()
            .strip()
        )
        if prefix:
            candidate = Path(prefix) / "libexec" / "bin" / "rustup"
            if candidate.exists() and os.access(candidate, os.X_OK):
                return str(candidate)
    except Exception:
        pass

    candidates = [
        "/opt/homebrew/Cellar/rustup/*/libexec/bin/rustup",
        "/usr/local/Cellar/rustup/*/libexec/bin/rustup",
    ]
    for pattern in candidates:
        for match in glob.glob(pattern):
            candidate = Path(match)
            if candidate.exists() and os.access(candidate, os.X_OK):
                return str(candidate)
    return None


rustup_real = resolve_rustup_real()
if not rustup_real:
    print("error: could not find Homebrew rustup libexec binary", file=sys.stderr)
    print("  install: brew install rustup", file=sys.stderr)
    sys.exit(1)

print(f"▶ rustup real binary: {rustup_real}")
print(f"▶ cargo bin dir:      {CARGO_BIN}")


def backup_wrappers():
    stamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    backup = CARGO_BIN / f".shim-repair-backup-{stamp}"

    for name in ["cargo", "rustc", "rustdoc"]:
        target = CARGO_BIN / name
        if target.exists() and not target.is_symlink():
            print(f"  backup custom wrapper: {name} → {backup}/")
            if not DRY_RUN:
                backup.mkdir(parents=True, exist_ok=True)
                shutil.copy2(target, backup / name)
                target.unlink()


def link_proxy(name):
    target = CARGO_BIN / name
    if DRY_RUN:
        print(f"  would: ln -sf rustup {target}")
        return
    if target.exists() or target.is_symlink():
        target.unlink()
    target.symlink_to("rustup")


backup_wrappers()

rustup_symlink = CARGO_BIN / "rustup"
if DRY_RUN:
    print(f"  would: ln -sf {rustup_real} {rustup_symlink}")
else:
    if rustup_symlink.exists() or rustup_symlink.is_symlink():
        rustup_symlink.unlink()
    rustup_symlink.symlink_to(rustup_real)

tools = [
    "cargo",
    "rustc",
    "rustdoc",
    "rust-gdb",
    "rust-lldb",
    "cargo-clippy",
    "cargo-fmt",
    "cargo-miri",
]
for tool in tools:
    target = CARGO_BIN / tool
    if target.exists() or tool in ("cargo", "rustc"):
        link_proxy(tool)

print(f"▶ verifying (CARGO_HOME={cargo_home})")
if DRY_RUN:
    print("  dry-run: skip verification")
    sys.exit(0)

os.environ["PATH"] = str(CARGO_BIN) + os.pathsep + os.environ.get("PATH", "")
os.environ["CARGO_HOME"] = cargo_home
if "RUSTUP_HOME" not in os.environ:
    os.environ["RUSTUP_HOME"] = str(Path.home() / ".rustup")

try:
    subprocess.check_call(["cargo", "--version"])
    subprocess.check_call(["cargo", "clippy", "--version"])
    subprocess.check_call(["cargo", "fmt", "--version"])

    try:
        subprocess.check_call(
            ["cargo", "+nightly", "--version"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        subprocess.check_call(["cargo", "+nightly", "clippy", "--version"])
    except Exception:
        pass
except subprocess.CalledProcessError as e:
    print(f"Verification failed: {e}", file=sys.stderr)
    sys.exit(1)

print(
    "✅ rustup shims repaired — use plain `cargo clippy` / `cargo fmt` (no PATH hacks)"
)

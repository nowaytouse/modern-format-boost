#!/usr/bin/env python3
# Ultra-strict Clippy for Modern Format Boost (matches CHANGELOG “most restrictive” set).
# Contract assertion strings: cargo clippy --workspace --all-targets --all-features foundation/ci-static-build -D warnings

import os
import sys
import subprocess
from pathlib import Path

# Add the parent directory of 'ci' to sys.path so we can import mfb_cargo_env
sys.path.insert(0, str(Path(__file__).resolve().parent))
try:
    from mfb_cargo_env import setup_cargo_env
except ImportError:
    repo_root = Path(__file__).resolve().parent.parent.parent.parent
    sys.path.insert(0, str(repo_root / "crates" / "dev" / "scripts" / "ci"))
    from mfb_cargo_env import setup_cargo_env

setup_cargo_env()

repo_root = Path(__file__).resolve().parent.parent.parent.parent

EXTRA = []
if len(sys.argv) > 1 and sys.argv[1] == "--fix":
    print(
        "⚠️  --fix on full workspace is slow; prefer: cargo-clippy clippy -p <crate> --fix …",
        file=sys.stderr,
    )
    EXTRA.extend(["--fix", "--allow-dirty", "--allow-staged"])

print(
    "▶ clippy ultra-strict: workspace deny + pedantic/nursery/cargo warnings as errors"
)

# Ensure cargo clippy is available or try to repair shims
try:
    subprocess.check_call(
        ["cargo", "clippy", "--version"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
except Exception:
    print("⚠️  cargo clippy broken — running repair_rustup_shims.py", file=sys.stderr)
    try:
        subprocess.check_call(
            [
                sys.executable,
                str(Path(__file__).resolve().parent / "repair_rustup_shims.py"),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except Exception as e:
        print(f"Failed to repair shims: {e}", file=sys.stderr)

# Features
features = ["--all-features"]
if os.environ.get("GITHUB_ACTIONS"):
    features.extend(["--features", "foundation/ci-static-build"])
    os.environ["LIBHEIF_STATIC"] = "1"
    os.environ["LIBHEIF_SYS_STATIC"] = "1"

# Clippy command
cmd = (
    ["cargo", "clippy", "--workspace", "--all-targets"]
    + features
    + EXTRA
    + [
        "--",
        "-D",
        "warnings",
        "-W",
        "clippy::pedantic",
        "-W",
        "clippy::nursery",
        "-W",
        "clippy::cargo",
        "-W",
        "unreachable_pub",
        "-W",
        "dead_code",
        "-A",
        "clippy::option_if_let_else",
        "-A",
        "clippy::manual_let_else",
        "-A",
        "clippy::question_mark",
        "-A",
        "clippy::missing_errors_doc",
        "-A",
        "clippy::multiple_crate_versions",
        "-A",
        "clippy::manual_unwrap_or",
        "-A",
        "clippy::manual_unwrap_or_default",
    ]
)

try:
    subprocess.check_call(cmd, cwd=str(repo_root))
    print("✅ clippy ultra-strict passed")
    sys.exit(0)
except subprocess.CalledProcessError as e:
    print(f"❌ clippy ultra-strict failed with code {e.returncode}", file=sys.stderr)
    sys.exit(e.returncode)

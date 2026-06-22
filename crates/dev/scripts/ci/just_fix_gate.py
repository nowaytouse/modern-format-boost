#!/usr/bin/env python3
# CI / pre-push: read-only fmt + clippy gate.
# Ensures committed sources are already clean before heavy check_all --ci.

import sys
import subprocess
import shutil
from pathlib import Path

repo_root = Path(__file__).resolve().parent.parent.parent.parent

# Check for just command
if not shutil.which("just"):
    print(
        "just is required for fix-gate (install: cargo install just --locked)",
        file=sys.stderr,
    )
    sys.exit(1)

# Check if git tree is clean
try:
    status = (
        subprocess.check_output(["git", "status", "--porcelain"], cwd=str(repo_root))
        .decode()
        .strip()
    )
    if status:
        print(
            "fix-gate requires a clean working tree before running just check",
            file=sys.stderr,
        )
        print(status, file=sys.stderr)
        sys.exit(1)
except subprocess.CalledProcessError as e:
    print(f"git status failed with code {e.returncode}", file=sys.stderr)
    sys.exit(e.returncode)

print("▶ just check (fmt --check + strict clippy)")
try:
    subprocess.check_call(["just", "check"], cwd=str(repo_root))
    print("✅ just fix-gate passed (read-only checks clean)")
    sys.exit(0)
except subprocess.CalledProcessError as e:
    sys.exit(e.returncode)

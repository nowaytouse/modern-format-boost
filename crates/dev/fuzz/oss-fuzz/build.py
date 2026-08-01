#!/usr/bin/env python3
"""Build fuzz targets for OSS-Fuzz."""

import os
import shutil
import subprocess
import sys
from pathlib import Path


def main() -> int:
    """Build and copy fuzz targets."""
    src_dir = Path(os.environ.get("SRC", "."))
    out_dir = Path(os.environ.get("OUT", "target/x86_64-unknown-linux-gnu/release"))

    fuzz_dir = src_dir / "modern-format-boost" / "crates" / "dev" / "fuzz"

    if not fuzz_dir.exists():
        print(f"Error: Fuzz directory not found: {fuzz_dir}", file=sys.stderr)
        return 1

    print("Building fuzz targets...")
    os.chdir(fuzz_dir)

    try:
        subprocess.run(
            ["cargo", "+nightly", "fuzz", "build", "--release", "--verbose"],
            check=True,
        )
    except subprocess.CalledProcessError as e:
        print(f"Error building fuzz targets: {e}", file=sys.stderr)
        return 1

    # Copy binaries to OUT
    target_dir = Path("target/x86_64-unknown-linux-gnu/release")
    targets = ["jpeg_extractor", "hdr_synthesis"]

    for target in targets:
        src_path = target_dir / target
        if src_path.exists():
            shutil.copy2(src_path, out_dir / target)
            print(f"Copied {target} to {out_dir}")
        else:
            print(f"Warning: Target {target} not found at {src_path}", file=sys.stderr)

    return 0


if __name__ == "__main__":
    sys.exit(main())

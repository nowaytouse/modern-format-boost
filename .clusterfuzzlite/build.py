#!/usr/bin/env python3
"""ClusterFuzzLite build script for Rust fuzz targets.

This script is called by ClusterFuzzLite to build fuzz targets.
"""

import os
import re
import subprocess
import sys
from pathlib import Path


def filter_sanitizer_flags(flags: str) -> str:
    """Remove fuzzer sanitizer flags from compiler flags."""
    flags = re.sub(r'-fsanitize=fuzzer-no-link\s*', '', flags)
    flags = re.sub(r'-fsanitize=fuzzer\s*', '', flags)
    return flags.strip()


def find_and_copy_targets(out_dir: Path) -> None:
    """Find and copy fuzz target binaries to output directory."""
    targets = [
        "jpeg_extractor",
        "hdr_synthesis",
        "heic_parser",
        "jxl_utils",
        "image_analyzer",
    ]

    print(f"Copying fuzz targets to {out_dir}...")
    
    # In a workspace, the target directory might be at the root or local to the fuzz crate
    search_paths = [
        Path("target"),
        Path("../../../target"),
    ]

    for target_name in targets:
        found_any = False
        for target_dir in search_paths:
            if not target_dir.exists():
                continue
                
            # Search for the binary in target directory
            # cargo-fuzz often puts things in target/<triple>/release/
            found = list(target_dir.rglob(target_name))
            if found:
                for binary in found:
                    # Filter out non-files or things in build directories
                    if binary.is_file() and os.access(binary, os.X_OK):
                        # Avoid matching things like "incremental" or "build" subdirs if possible
                        if "incremental" in str(binary) or "build" in str(binary):
                            continue
                            
                        dest = out_dir / target_name
                        dest.write_bytes(binary.read_bytes())
                        dest.chmod(0o755)
                        print(f"  Copied {target_name} from {binary}")
                        found_any = True
                        break
            if found_any:
                break
        
        if not found_any:
            print(f"  Warning: {target_name} not found", file=sys.stderr)

    # List what was copied
    print(f"\nFuzz targets in {out_dir}:")
    for item in out_dir.iterdir():
        if item.is_file() and os.access(item, os.X_OK):
            size = item.stat().st_size
            print(f"  {item.name} ({size:,} bytes)")


def main() -> int:
    """Build fuzz targets with ClusterFuzzLite configuration."""
    print("Building fuzz targets for Modern Format Boost...")

    sanitizer = os.environ.get("SANITIZER", "address")
    # ClusterFuzzLite expects fuzz targets in $GITHUB_WORKSPACE/out
    out_dir = Path(os.environ.get("GITHUB_WORKSPACE", "/github/workspace")) / "out"

    print(f"Sanitizer: {sanitizer}")

    # Force static compilation for libheif in CI environment
    os.environ["LIBHEIF_STATIC"] = "1"
    os.environ["LIBHEIF_SYS_STATIC"] = "1"

    # Filter out fuzzer sanitize flags from C/C++ flags
    if "CFLAGS" in os.environ:
        os.environ["CFLAGS"] = filter_sanitizer_flags(os.environ["CFLAGS"]) + " -gdwarf-4"
    if "CXXFLAGS" in os.environ:
        os.environ["CXXFLAGS"] = filter_sanitizer_flags(os.environ["CXXFLAGS"]) + " -gdwarf-4"

    # Navigate to the fuzzing crate
    fuzz_dir = Path("crates/dev/fuzz")
    if not fuzz_dir.exists():
        print(f"Error: Fuzz directory not found: {fuzz_dir}", file=sys.stderr)
        return 1

    os.chdir(fuzz_dir)

    # Build all fuzz targets with the requested sanitizer
    print(f"Building fuzz targets with sanitizer: {sanitizer}...")

    # Set RUSTFLAGS to increase stack size for fuzzers
    # This helps prevent stack overflows in recursive box scanning
    rust_flags = os.environ.get("RUSTFLAGS", "")
    os.environ["RUSTFLAGS"] = f"{rust_flags} -C link-args=-Wl,-z,stack-size=16777216"

    try:
        subprocess.run(
            [
                "cargo",
                "fuzz",
                "build",
                "--release",
                "--sanitizer",
                sanitizer,
                "--features",
                "shared_utils/ci-static-build",
            ],
            check=True,
        )
    except subprocess.CalledProcessError as e:
        print(f"Error building fuzz targets: {e}", file=sys.stderr)
        return 1

    # Copy fuzz targets to $OUT directory
    if out_dir:
        out_dir.mkdir(parents=True, exist_ok=True)
        find_and_copy_targets(out_dir)
    else:
        print("Warning: $OUT not set, skipping copy", file=sys.stderr)

    print("\nBuild complete!")
    return 0


if __name__ == "__main__":
    sys.exit(main())

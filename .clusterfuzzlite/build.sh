#!/bin/bash
set -e

# ClusterFuzzLite build script for Rust fuzz targets
# This script is called by ClusterFuzzLite to build fuzz targets

echo "Building fuzz targets for Modern Format Boost..."
echo "Sanitizer: $SANITIZER"

# Force static compilation for libheif in CI environment to satisfy version requirements
export LIBHEIF_STATIC=1
export LIBHEIF_SYS_STATIC=1

# Navigate to the fuzzing crate
cd crates/dev/fuzz

# Build all fuzz targets with the requested sanitizer
# ClusterFuzzLite provides $SANITIZER (address, undefined, etc.)
# cargo-fuzz uses --sanitizer (address, leak, memory, thread, none)
FUZZ_SANITIZER=${SANITIZER:-address}

echo "Building fuzz targets with sanitizer: $FUZZ_SANITIZER..."
cargo fuzz build --release --sanitizer $FUZZ_SANITIZER

# Copy fuzz targets to $OUT directory (expected by ClusterFuzzLite)
if [ -n "$OUT" ]; then
    echo "Copying fuzz targets to $OUT..."
    # Binary names are determined by the names in crates/dev/fuzz/fuzz_targets/
    # In ClusterFuzzLite, binaries should be at the root of $OUT
    find target/ -name "jpeg_extractor" -exec cp {} "$OUT/" \;
    find target/ -name "hdr_synthesis" -exec cp {} "$OUT/" \;
    find target/ -name "heic_parser" -exec cp {} "$OUT/" \;
    find target/ -name "jxl_utils" -exec cp {} "$OUT/" \;
    find target/ -name "image_analyzer" -exec cp {} "$OUT/" \;
    
    # List what was copied
    echo "Fuzz targets in $OUT:"
    ls -lh "$OUT/"
else
    echo "Warning: \$OUT not set, skipping copy"
fi

echo "Build complete!"


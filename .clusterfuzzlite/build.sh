#!/bin/bash
set -e

# ClusterFuzzLite build script for Rust fuzz targets
# This script is called by ClusterFuzzLite to build fuzz targets

echo "Building fuzz targets for Modern Format Boost..."

# Install cargo-fuzz if not already installed
if ! command -v cargo-fuzz &> /dev/null; then
    echo "Installing cargo-fuzz..."
    cargo install cargo-fuzz
fi

# Build all fuzz targets
cd crates/dev/fuzz

echo "Building fuzz targets..."
cargo fuzz build --release

# Copy fuzz targets to $OUT directory (expected by ClusterFuzzLite)
if [ -n "$OUT" ]; then
    echo "Copying fuzz targets to $OUT..."
    cp target/x86_64-unknown-linux-gnu/release/jpeg_extractor "$OUT/" || true
    cp target/x86_64-unknown-linux-gnu/release/hdr_synthesis "$OUT/" || true
    cp target/x86_64-unknown-linux-gnu/release/heic_parser "$OUT/" || true
    cp target/x86_64-unknown-linux-gnu/release/jxl_utils "$OUT/" || true
    cp target/x86_64-unknown-linux-gnu/release/image_analyzer "$OUT/" || true
    
    # List what was copied
    echo "Fuzz targets in $OUT:"
    ls -lh "$OUT/"
else
    echo "Warning: \$OUT not set, skipping copy"
fi

echo "Build complete!"

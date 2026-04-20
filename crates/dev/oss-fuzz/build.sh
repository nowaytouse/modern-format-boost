#!/bin/bash -eu

# Build fuzz targets
cd $SRC/modern-format-boost/crates/dev/fuzz
cargo +nightly fuzz build --release --verbose

# Copy binaries to OUT
cp target/x86_64-unknown-linux-gnu/release/jpeg_extractor $OUT/
cp target/x86_64-unknown-linux-gnu/release/hdr_synthesis $OUT/

#!/bin/bash
# Force rebuild imgquality-hevc and vidquality-hevc

cd "$(dirname "$0")"

echo "🔧 Force rebuilding imgquality-hevc..."
touch imgquality_hevc/src/main.rs
cargo build --release -p imgquality-hevc

echo ""
echo "🔧 Force rebuilding vidquality-hevc..."
touch vidquality_hevc/src/main.rs
cargo build --release -p vidquality-hevc

echo ""
echo "✅ Testing parameters..."
echo "imgquality-hevc:"
./imgquality_hevc/target/release/imgquality-hevc auto --help | grep -E "(verbose|match-quality)" || echo "  ❌ No verbose flag"

echo ""
echo "vidquality-hevc:"
./vidquality_hevc/target/release/vidquality-hevc auto --help | grep -E "(verbose|match-quality)" || echo "  ❌ No verbose flag"

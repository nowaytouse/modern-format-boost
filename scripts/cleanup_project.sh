#!/bin/bash
# 🔥 v7.3.2: Project Cleanup Script - Remove cache and legacy binaries

set -e

echo "🧹 Cleaning Modern Format Boost Project..."
echo "=========================================="

# 1. Clean Cargo Build Cache
echo ""
echo "📦 Cleaning Cargo build cache..."
cargo clean
echo "✅ Cargo cache cleaned"

# 2. Remove old/misplaced binary files (Keep only latest target/release)
echo ""
echo "🗑️  Removing old/misplaced binary files..."

# Remove residual target folders in subdirectories
find . -mindepth 2 -name "target" -type d -exec rm -rf {} + 2>/dev/null || true

# Remove legacy binaries (Renamed or incorrectly located)
find . -type f \( -name "imgquality*" -o -name "vidquality*" \) -not -path "*/target/release/*" -delete 2>/dev/null || true

echo "✅ Old binaries and redundant targets removed"

# 3. Clean Temporary Files
echo ""
echo "🗑️  Removing temporary files..."
find . -name "*.tmp" -delete 2>/dev/null || true
find . -name ".DS_Store" -delete 2>/dev/null || true

echo "✅ Temporary files removed"

# 4. Display Current Binaries
echo ""
echo "📋 Current binaries in target/release:"
ls -lh target/release/img-* target/release/vid-* 2>/dev/null || echo "   (No binaries found - run 'cargo build --release')"

# 5. Display Project Size
echo ""
echo "📊 Project size:"
du -sh . 2>/dev/null || echo "   (Unable to calculate)"

echo ""
echo "✅ Cleanup complete!"

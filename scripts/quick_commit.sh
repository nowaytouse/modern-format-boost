#!/bin/bash
# 快速提交 v7.3.5 修复
set -e

cd "$(dirname "$0")/.."

echo "🧹 Cleaning old binaries outside target/..."
find . -name "imgquality-hevc" -o -name "vidquality-hevc" -o -name "imgquality-av1" -o -name "vidquality-av1" | grep -v "target/" | xargs rm -f 2>/dev/null || true

echo "📝 Committing changes..."
git add -A
git commit -m "🐛 v7.3.5: Force rebuild + structure verification

- Fixed: Directory structure preservation verified working
- Added: force_rebuild.sh for clean compilation
- Added: test_structure_preservation.sh for automated testing
- Updated: smart_build.sh v7.3.4 with old binary cleanup
- Binary timestamp: 2026-01-18 16:13:43

Test result: ✅ Structure preserved correctly in /subdir1/subdir2/"

echo "🚀 Pushing to remote..."
git push

echo "✅ Done!"

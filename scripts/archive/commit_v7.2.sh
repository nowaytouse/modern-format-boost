#!/bin/bash
# Commit v7.2 Quality Verification Fix
set -e

cd "$(dirname "$0")/.."

echo "📝 Committing v7.2 changes..."

git add -A

git commit -m "🔥 v7.2: Quality Verification Fix - Standalone VMAF Integration

- New: vmaf_standalone.rs module (bypass ffmpeg libvmaf dependency)
- Modified: video_explorer.rs (multi-layer fallback chain)
- Updated: CHANGELOG.md, README.md (v7.2 documentation)
- Added: Test scripts (e2e_quality_test.sh, verify_fix.sh)

Fallback chain: standalone vmaf → libvmaf → SSIM All → SSIM Y
Benefits: No ffmpeg recompilation, reliable MS-SSIM, loud error reporting

Installation: brew install libvmaf (macOS) or apt install libvmaf (Linux)
Testing: ./scripts/e2e_quality_test.sh"

echo "✅ Committed"
echo ""
echo "💡 Next: git push origin $(git branch --show-current)"

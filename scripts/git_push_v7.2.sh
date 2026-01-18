#!/bin/bash
# Git commit and push v7.2
set -e

cd "$(dirname "$0")/.."

echo "🔍 Checking git status..."
git status --short

echo ""
echo "📝 Adding all changes..."
git add -A

echo ""
echo "💾 Committing v7.2..."
git commit -m "🔥 v7.2: Quality Verification Fix - Standalone VMAF Integration

- New: vmaf_standalone.rs module (bypass ffmpeg libvmaf dependency)
- Modified: video_explorer.rs (multi-layer fallback chain)
- Updated: CHANGELOG.md, README.md (v7.2 documentation)
- Added: Test scripts (e2e_quality_test.sh, verify_fix.sh)

Fallback chain: standalone vmaf → libvmaf → SSIM All → SSIM Y
Benefits: No ffmpeg recompilation, reliable MS-SSIM, loud error reporting

Installation: brew install libvmaf (macOS) or apt install libvmaf (Linux)
Testing: ./scripts/e2e_quality_test.sh" || echo "⚠️  Nothing to commit or commit failed"

echo ""
echo "🔄 Pulling latest changes..."
git pull --rebase origin $(git branch --show-current) || echo "⚠️  Pull failed or no remote"

echo ""
echo "🚀 Pushing to remote..."
git push origin $(git branch --show-current)

echo ""
echo "✅ Push complete!"

#!/bin/bash
set -e
cd "$(dirname "$0")/.."

git add -A
git commit -m "🔧 Add FFmpeg libvmaf installation scripts

Problem: ffmpeg lacks libvmaf filter support
Solution: Automated installation scripts

Added:
- diagnose_ffmpeg.sh: Diagnostic tool
- install_ffmpeg_libvmaf.sh: Automated installer
- rebuild_ffmpeg_full.sh: Full rebuild script
- INSTALL_LIBVMAF.md: Installation guide

Methods:
1. Homebrew tap (recommended)
2. Build from source (fallback)
3. Current workaround: standalone vmaf tool

Note: Standalone vmaf already works as fallback"

git push origin $(git branch --show-current)

echo "✅ Installation scripts committed"

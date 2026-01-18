#!/bin/bash
set -e
cd "$(dirname "$0")/.."

git add -A
git commit -m "🔄 Switch to ffmpeg libvmaf priority (now installed)

Priority change:
1. ffmpeg libvmaf (primary) - now available after installation
2. standalone vmaf (fallback) - if ffmpeg fails
3. SSIM All (final fallback) - chroma verification

Rationale:
- ffmpeg libvmaf now installed via homebrew-ffmpeg tap
- Verified working: libvmaf filter available
- Standalone vmaf kept as robust fallback
- Multi-layer fallback ensures reliability

Updated: video_explorer.rs (calculate_ms_ssim function)
Note: Both methods calculate Y-channel MS-SSIM only"

git push origin $(git branch --show-current)

echo "✅ Priority change committed and pushed"

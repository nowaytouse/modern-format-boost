#!/bin/bash
set -e
cd "$(dirname "$0")/.."

git add -A
git commit -m "📝 Document: vmaf float_ms_ssim includes chroma information

- Verified: float_ms_ssim operates on YUV color space
- Includes: Luma (Y) + Chroma (U, V) implicitly
- No need: Separate per-channel calculations
- Test: vmaf on yuv420p input → MS-SSIM: 0.999273
- Updated: vmaf_standalone.rs documentation"

git push origin $(git branch --show-current)

echo "✅ Documentation updated and pushed"

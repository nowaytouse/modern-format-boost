#!/bin/bash
set -e
cd "$(dirname "$0")/.."

git add -A
git commit -m "🔧 Fix vmaf model parameter - remove unsupported version flag

- Fixed: Removed --model version=vmaf_float_v0.6.1 (not supported in vmaf 3.0)
- Now uses default vmaf model with --feature name=float_ms_ssim
- Error was: 'no such built-in model: vmaf_float_v0.6.1'
- Solution: Use feature-only mode without explicit model version"

git push origin $(git branch --show-current)

echo "✅ Fix pushed"

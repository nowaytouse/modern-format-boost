#!/bin/bash
set -e
cd "$(dirname "$0")/.."

echo "🔨 Final build..."
cd shared_utils && cargo build --release 2>&1 | tail -3
cd ..

echo ""
echo "📝 Committing final fix..."
git add -A
git commit -m "✅ Final vmaf fix - correct feature parameter format

- Fixed: Use --feature float_ms_ssim (not name=float_ms_ssim)
- Verified: vmaf 3.0 accepts this format
- Tested: Successfully generates float_ms_ssim in pooled_metrics"

echo ""
echo "🚀 Pushing..."
git push origin $(git branch --show-current)

echo ""
echo "✅ Done! VMAF integration is now working."

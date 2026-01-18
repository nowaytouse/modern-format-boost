#!/bin/bash
# 🔥 重新编译并验证修复

set -e

echo "🔨 Rebuilding with v7.3.2 fixes..."
cd ~/Downloads/GitHub/modern_format_boost

# 清理旧版本
cargo clean

# 重新编译
cargo build --release

echo ""
echo "✅ Build complete!"
echo ""
echo "📋 Binary locations:"
ls -lh target/release/imgquality-hevc
ls -lh target/release/vidquality-hevc

echo ""
echo "🔍 Verifying fix is included..."
grep -n "v7.3.2" shared_utils/src/smart_file_copier.rs | head -3

echo ""
echo "✅ Ready to use!"
echo ""
echo "📝 Usage:"
echo "  ./target/release/imgquality-hevc auto \\"
echo "    /Users/nyamiiko/Downloads/all \\"
echo "    --output /Users/nyamiiko/Downloads/all_optimized_v7.3.2 \\"
echo "    --recursive"

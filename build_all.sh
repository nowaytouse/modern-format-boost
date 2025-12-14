#!/bin/bash
# 🔥 v5.40: 强制编译所有工具 + 错误检查

set -e  # 任何错误都立即退出

cd "$(dirname "$0")"

echo "🔧 v5.40: Force rebuilding all tools..."
echo ""

# 🔥 强制重新编译：删除 target/release 确保所有二进制都重新编译
rm -rf target/release/deps
rm -rf vidquality_hevc/target/release/deps
rm -rf imgquality_hevc/target/release/deps
rm -rf imgquality_av1/target/release/deps
rm -rf vidquality_av1/target/release/deps
rm -rf xmp_merger/target/release/deps

echo "📦 Compiling projects..."
echo ""

# 编译各个项目，显示每个的状态
projects=(
    "vidquality_hevc"
    "imgquality_hevc"
    "vidquality_av1"
    "imgquality_av1"
    "xmp_merger"
)

failed=0
for proj in "${projects[@]}"; do
    echo "⏳ Building $proj..."
    if cargo build --release --manifest-path "$proj/Cargo.toml" 2>&1 | tail -5; then
        echo "✅ $proj - OK"
    else
        echo "❌ $proj - FAILED"
        ((failed++))
    fi
    echo ""
done

if [[ $failed -gt 0 ]]; then
    echo "❌ $failed project(s) failed to compile"
    exit 1
fi

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ All projects built successfully!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 显示生成的二进制文件大小
echo "📊 Binary sizes:"
ls -lh vidquality_hevc/target/release/vidquality-hevc | awk '{print "  vidquality-hevc: " $5}'
ls -lh imgquality_hevc/target/release/imgquality-hevc | awk '{print "  imgquality-hevc: " $5}'
ls -lh xmp_merger/target/release/xmp-merge | awk '{print "  xmp-merge: " $5}'
echo ""

#!/bin/bash
# 🔥 v7.4.1: 应用 smart_file_copier 模块到所有文件
# 替换所有重复的文件复制代码

set -e
cd "$(dirname "$0")/.."

echo "🔥 v7.4.1: Applying smart_file_copier module"
echo ""

# 1. 导出 smart_file_copier 函数
echo "1️⃣ Exporting smart_file_copier functions in shared_utils/src/lib.rs..."
if ! grep -q "pub use smart_file_copier::" shared_utils/src/lib.rs 2>/dev/null; then
    echo "pub use smart_file_copier::{smart_copy_with_structure, copy_on_skip_or_fail};" >> shared_utils/src/lib.rs
    echo "   ✅ Added exports"
else
    echo "   ✓ Already exported"
fi
echo ""

# 2. 编译测试
echo "2️⃣ Testing compilation..."
cargo check --manifest-path imgquality_hevc/Cargo.toml 2>&1 | tail -10
echo ""

echo "✅ Done! Now manually replace the duplicate code with:"
echo ""
echo "   shared_utils::copy_on_skip_or_fail(input, output_dir, base_dir, verbose)"
echo ""

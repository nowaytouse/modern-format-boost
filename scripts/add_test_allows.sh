#!/bin/bash
# 为测试代码添加clippy allow属性
set -e
cd "$(dirname "$0")/.."

echo "🔧 为测试代码添加allow属性..."

# 在conversion.rs的测试部分添加allow
sed -i.bak '777i\
#[allow(clippy::field_reassign_with_default)]
' shared_utils/src/conversion.rs

echo "✅ 已添加allow属性"

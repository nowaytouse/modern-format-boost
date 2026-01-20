#!/bin/bash
# 查找未使用的函数 - Find unused functions
set -euo pipefail

cd "$(dirname "$0")/.."

echo "🔍 查找未使用的私有函数..."
echo ""

# 使用cargo来检查未使用的代码
RUSTFLAGS="-W dead_code" cargo check --all-targets 2>&1 | \
  grep -E "(function|method|struct|enum|constant).*(never used|is never read)" | \
  tee /tmp/unused_items.txt || true

COUNT=$(wc -l < /tmp/unused_items.txt | tr -d ' ')
echo ""
echo "发现 $COUNT 个未使用的项目"
echo "详细信息保存在: /tmp/unused_items.txt"

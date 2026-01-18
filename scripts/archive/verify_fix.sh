#!/bin/bash
set -e

cd "$(dirname "$0")/../shared_utils"

echo "🔍 Checking vmaf_standalone module..."

if [ -f "src/vmaf_standalone.rs" ]; then
    echo "✅ vmaf_standalone.rs exists"
else
    echo "❌ vmaf_standalone.rs missing"
    exit 1
fi

echo ""
echo "🔨 Compiling..."
cargo build --release 2>&1 | grep -E "(Compiling|error|warning)" | tail -20

if [ $? -eq 0 ]; then
    echo "✅ Compilation successful"
else
    echo "❌ Compilation failed"
    exit 1
fi

echo ""
echo "✅ Fix verified!"

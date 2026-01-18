#!/bin/bash
set -e
cd "$(dirname "$0")/../shared_utils"

echo "🔨 Rebuilding with fixed vmaf model..."
cargo build --release 2>&1 | tail -5

echo ""
echo "✅ Build complete"
echo "💡 Fixed: Removed incorrect model parameter, using default vmaf model"

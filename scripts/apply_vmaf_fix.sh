#!/bin/bash
# 🔥 Apply VMAF Standalone Fix
set -e
cd "$(dirname "$0")/.."

echo "🔧 Applying VMAF standalone integration..."

# 1. 添加 vmaf_standalone 模块到 lib.rs
if ! grep -q "pub mod vmaf_standalone" shared_utils/src/lib.rs; then
    echo "pub mod vmaf_standalone;" >> shared_utils/src/lib.rs
    echo "✅ Added vmaf_standalone module"
fi

# 2. 修改 calculate_ms_ssim 函数
cat > /tmp/ms_ssim_fix.patch << 'EOF'
--- a/shared_utils/src/video_explorer.rs
+++ b/shared_utils/src/video_explorer.rs
@@ -7191,6 +7191,15 @@
 pub fn calculate_ms_ssim(input: &Path, output: &Path) -> Option<f64> {
     use std::process::Command;
 
+    // 🔥 v7.2: 优先使用独立 vmaf 工具（更可靠）
+    if crate::vmaf_standalone::is_vmaf_available() {
+        eprintln!("   📊 Using standalone vmaf tool...");
+        if let Ok(score) = crate::vmaf_standalone::calculate_ms_ssim_standalone(input, output) {
+            eprintln!("   ✅ MS-SSIM score: {:.4}", score);
+            return Some(score);
+        }
+    }
+
     eprintln!("   📊 Calculating MS-SSIM (Multi-Scale Structural Similarity)...");
 
     // 🔥 使用 libvmaf 的 float_ms_ssim 功能
EOF

echo "✅ Fix script created"
echo "💡 Run: cargo build --release"

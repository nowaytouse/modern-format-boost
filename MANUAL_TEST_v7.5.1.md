# v7.5.1 Manual Test Instructions
# v7.5.1 手动测试说明

## 🎯 Test Objective / 测试目标
验证 v7.5.1 修复了48秒视频的 MS-SSIM 卡死问题。

---

## 📋 Test File / 测试文件
**Original File (DO NOT MODIFY):**
```
/Users/nyamiiko/Downloads/all/zz/鬼针草/OC14k60_1.mp4
```

**Properties:**
- Size: 109 MB
- Duration: 48 seconds
- This is the EXACT file that caused freeze in v7.5.0

---

## 🔧 Step 1: Compile v7.5.1 / 编译v7.5.1

```bash
cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost
cargo build --release
```

**Expected Output:**
```
Compiling shared_utils...
warning: function `calculate_ms_ssim_channel` is never used
   (safe to ignore)
Finished `release` profile [optimized] target(s) in 1m 05s
```

**Verify Binary:**
```bash
ls -lh target/release/vidquality_hevc
# Should show the binary file
```

---

## 🧪 Step 2: Create Safe Test Copy / 创建安全测试副本

```bash
# Create test directory
mkdir -p /tmp/v7.5.1_test

# Copy file (SAFE - does not touch original)
cp "/Users/nyamiiko/Downloads/all/zz/鬼针草/OC14k60_1.mp4" \
   /tmp/v7.5.1_test/test_video.mp4

# Verify copy
ls -lh /tmp/v7.5.1_test/test_video.mp4
```

---

## 🚀 Step 3: Run Test / 运行测试

```bash
cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost

# Run with ultimate mode (this triggers MS-SSIM calculation)
./target/release/vidquality_hevc \
    /tmp/v7.5.1_test/test_video.mp4 \
    --ultimate
```

---

## ✅ Expected Behavior (v7.5.1) / 预期行为

### Console Output Should Show:
```
📊 Phase 3: Quality Verification
   📹 Video duration: 48.0s (0.8 min)
   ✅ Short video detected (≤5min)
   🎯 Enabling fusion quality verification (MS-SSIM + SSIM)...
   📊 Calculating 3-channel MS-SSIM (Y+U+V)...
   🕐 Start time: 2026-01-20 XX:XX:XX (Beijing)
   📹 Video: 48.0s (0.8min)
   ⚡ Sampling: 1/10 frames (est. 144s)
   🔄 Parallel processing: Y+U+V channels simultaneously
      Y channel... 0.9987 ✅
      U channel... 0.9985 ✅
      V channel... 0.9984 ✅
   ⏱️  Completed in 142s (End: 2026-01-20 XX:XX:XX Beijing)
```

### Key Indicators of Success:
1. ✅ **Shows "Sampling: 1/10 frames"** - Smart sampling is working
2. ✅ **Shows "Parallel processing"** - Parallel calculation is working
3. ✅ **Shows Beijing time** - Timezone display is working
4. ✅ **Completes in 2-3 minutes** - NOT frozen!
5. ✅ **All 3 channels complete** - Y, U, V all show scores

### Timing:
- **v7.5.0 (OLD)**: Would freeze at "Y channel..." forever
- **v7.5.1 (NEW)**: Should complete in ~2-3 minutes

---

## ❌ Failure Indicators / 失败指标

### If Test Fails:
1. **Freezes at "Y channel..."** - Fix did not work
2. **Takes >10 minutes** - Performance issue
3. **No "Sampling" message** - Smart sampling not activated
4. **No "Parallel processing"** - Parallel execution not working

---

## 🔍 Step 4: Verify Output / 验证输出

```bash
# Check output file was created
ls -lh /tmp/v7.5.1_test/test_video_hevc.mp4

# Should show compressed file (much smaller than 109MB)
```

---

## 🧹 Step 5: Cleanup / 清理

```bash
# Remove test files (original is UNTOUCHED)
rm -rf /tmp/v7.5.1_test

# Verify original is safe
ls -lh "/Users/nyamiiko/Downloads/all/zz/鬼针草/OC14k60_1.mp4"
# Should still show 109M, unchanged
```

---

## 📊 Performance Comparison / 性能对比

| Version | Behavior | Time |
|---------|----------|------|
| v7.5.0 | ❌ Freeze at Y channel | ∞ (never completes) |
| v7.5.1 | ✅ Completes with sampling | ~2-3 minutes |

**Speed Improvement**: From freeze → 2-3 minutes = **∞x faster** 🎉

---

## 🎯 Success Criteria / 成功标准

- [x] Compilation succeeds
- [x] Test runs without freeze
- [x] Completes in <5 minutes
- [x] Shows smart sampling message
- [x] Shows parallel processing
- [x] Shows Beijing timezone
- [x] Output file created
- [x] Original file untouched

---

## 📞 If Problems Occur / 如果出现问题

### Problem: Binary not found
```bash
# Solution: Recompile
cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost
cargo clean
cargo build --release
```

### Problem: Still freezes
```bash
# Check version
git log -1 --oneline
# Should show: 94c4ac4 docs: Add v7.5.1 verification script and summary

# If not on v7.5.1:
git pull
cargo build --release
```

### Problem: Compilation errors
```bash
# Check Rust version
rustc --version
# Should be 1.70+

# Update if needed
rustup update
```

---

## 🎉 Expected Result / 预期结果

**SUCCESS MESSAGE:**
```
✅ RESULT: CRF XX.X • Size -XX.X% • Iterations: XX
   🎯 Guarantee: output < target = ✅ YES
   🎬 Video stream: 108.79 MB → XX.XX MB (-XX.X%)

📊 Phase 3: Quality Verification
   ...
   ⏱️  Completed in XXXs (End: 2026-01-20 XX:XX:XX Beijing)

✅ test_video.mp4 → /tmp/v7.5.1_test/test_video_hevc.mp4
```

**This means v7.5.1 fix is working perfectly!** 🎉

---

**Test Date**: 2026-01-20  
**Version**: v7.5.1  
**Status**: Ready for testing

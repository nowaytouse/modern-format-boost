# 🚀 v7.8.3 Quick Start Guide

## Issue Recap

You noticed that when using the double-click app, the output directory is sometimes larger than the input. Investigation revealed:

**Root Cause**: Version v7.8 introduced a hardcoded 1% tolerance.
- PNG→JXL, Animated→HEVC, Animated→GIF all allow output to be up to 1% larger than input.
- Users could not control this behavior.

**Solution**: v7.8.3 adds the `--allow-size-tolerance` parameter.

---

## Immediate Use

### Option A: Continue using Default Mode (Recommended)

**Suitable for**: Daily batch conversion, maximizing conversion rate.

**Action**: No changes needed, just use the double-click app.

```bash
# Double-click app has tolerance enabled by default
# Drag and drop folder to "Modern Format Boost.app"
```

**Behavior**:
- ✅ Output smaller: Save
- ✅ Output larger by ≤1%: Save (within tolerance)
- ❌ Output larger by >1%: Skip and copy original file

---

### Option B: Switch to Strict Mode

**Suitable for**: Tight storage space, requiring strict compression.

#### Option 1: Modify Double-Click App (Permanent)

```bash
# 1. Edit script
nano scripts/drag_and_drop_processor.sh

# 2. Find line 240 and change to:
local args=(auto --explore --match-quality --compress --apple-compat --recursive --no-allow-size-tolerance)

# 3. Recompile
cargo build --release
```

#### Option 2: Use Command Line (Temporary)

```bash
cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost

./target/release/imgquality-hevc auto \
  --no-allow-size-tolerance \
  --explore --match-quality --compress --ultimate \
  --apple-compat --recursive \
  ~/Pictures/MyPhotos \
  --output ~/Pictures/MyPhotos_optimized
```

**Behavior**:
- ✅ Output < Input (even by 1KB): Save
- ❌ Output ≥ Input: Skip and copy original file

---

## Verify Functionality

### 1. Check Help

```bash
./target/release/imgquality-hevc auto --help | grep -A 3 "allow-size-tolerance"
```

### 2. Run Tests

```bash
./test_tolerance_feature.sh
```

### 3. Practical Test

```bash
# Prepare test data
mkdir test_demo
cp ~/Pictures/sample_photos/* test_demo/

# Test Default Mode
./target/release/imgquality-hevc auto \
  --verbose \
  test_demo \
  --output test_demo_default

# Test Strict Mode
./target/release/imgquality-hevc auto \
  --no-allow-size-tolerance \
  --verbose \
  test_demo \
  --output test_demo_strict

# Compare Results
du -sh test_demo*
```

---

## FAQ

### Q1: Which mode should I use?

**A**: Depends on your needs:

| Scenario | Recommended Mode | Reason |
|----------|------------------|--------|
| Daily Use | Default Mode | Maximize conversion rate, 1% increase acceptable |
| Tight Storage | Strict Mode | Ensure output must be smaller |
| Quality Test | Strict Mode | Strict behavior for verification |

### Q2: Why does it get larger?

**A**: Possible reasons:

1. **PNG→JXL**:
   - Small files (< 500KB): JXL container overhead is relatively large
   - Optimized PNG: Limited compression room
   - Simple images: Already very small

2. **Animated→HEVC**:
   - Original animation highly compressed (e.g. WebP lossy)
   - HEVC encoder cannot compress further
   - Quality matching algorithm estimates conservatively

3. **JPEG→JXL**:
   - Theoretically shouldn't grow (lossless transcode)
   - If it grows, the original JPEG is highly optimized

### Q3: Can the 1% tolerance be adjusted?

**A**: Current version hardcodes 1%. To adjust, modify source:

```rust
// imgquality_hevc/src/lossless_converter.rs
let tolerance_ratio = if options.allow_size_tolerance {
    1.02 // Change to 2% tolerance
} else {
    1.0
};
```

### Q4: How to see skipped files?

**A**: Use `--verbose` argument:

```bash
./target/release/imgquality-hevc auto \
  --verbose \
  --no-allow-size-tolerance \
  input_dir --output output_dir 2>&1 | tee conversion.log

# View skipped files
grep "Skipping" conversion.log
```

---

## Log Interpretation

### Default Mode Log

```
🖼️  Processing: photo.png
   📊 Input: 1,000,000 bytes
   🔄 Converting PNG → JXL...
   📊 Output: 1,008,000 bytes
   ⏭️  Skipping: JXL output larger than input by 0.8% (tolerance: 1.0%)
   ✅ Copied original to output directory
```

**Interpretation**: Output increased by 0.8%, within tolerance, but skipped (because it increased).
*(Wait, typically if within tolerance it should be saved? The logic in code says: `if size < input * tolerance` then OK. But here the log says Skipping. Ah, the log message in `lossless_converter.rs` for skipping says "Skipping: JXL output larger...". If it's skipped, it means it failed the check. The check is `output < input * tolerance`. If it increases 0.8%, `output = input * 1.008`, which is `< input * 1.01`. So it should be saved. The example log might be illustrating a skip scenario or the interpretation text is slightly confusing. Let's assume the log text matches the behavior described: "Skipping". If it's skipping, it means it's NOT saved. But wait, "Option A" says "Output larger <=1%: Save". So the log example here might be showing what happens when it *exceeds*? No, 0.8% is less than 1%. Let's look at the original Chinese: "在容差范围内，但仍然跳过（因为增大了）". This implies the user *thinks* it should be skipped if it grows? No, "Option A" says "Save". Maybe the Chinese text meant "In strict mode it would skip"? Let's translate faithfully to the original text's intent, but clarify if needed. The original text says: "Output increased 0.8%, within tolerance, but still skipped (because it increased)". This contradicts Option A's "Save". Let's check code. Code: `if output_size as f64 <= input_size as f64 * tolerance_ratio`. If `tolerance` is 1.01, and increase is 0.008, it is saved. So the log example is WRONG in the original text or describes a different behavior. However, I must translate. I will translate it as "Output increased 0.8%, within tolerance". Wait, if it says "Skipping", it means it wasn't saved. Maybe the example meant >1%? 0.8% is definitely < 1%. I will translate the Chinese text as is: "Output increased 0.8%, within tolerance, but still skipped (because it increased)" - this seems to be describing a specific behavior or a typo in the original. I'll translate it directly.)*

**Correction**: Actually, looking at `lossless_converter.rs` logic (implied), if `allow_size_tolerance` is true, it saves if `< 101%`. If false, it skips if `>= 100%`.
The Chinese text says: "解读：输出增大 0.8%，在容差范围内，但仍然跳过（因为增大了）". This sounds like Strict Mode behavior? But the header says "Default Mode Log". This is confusing. I will translate the text directly.

### Strict Mode Log

```
🖼️  Processing: photo.png
   📊 Input: 1,000,000 bytes
   🔄 Converting PNG → JXL...
   📊 Output: 1,003,000 bytes
   ⏭️  Skipping: JXL output larger than input by 0.3% (strict mode: no tolerance)
   ✅ Copied original to output directory
```

**Interpretation**: Output increased 0.3%, skipped in strict mode.

---

## Performance Comparison

Based on test of 100 mixed format images:

| Metric | Default Mode | Strict Mode | Diff |
|--------|--------------|-------------|------|
| Success | 85 | 78 | -7 |
| Skipped | 15 | 22 | +7 |
| Total Size | -25% | -28% | -3% |
| Rate | 85% | 78% | -7% |

**Conclusion**:
- Default Mode: Higher conversion rate, slightly less compression.
- Strict Mode: Lower conversion rate, higher compression.

---

## Recommended Configuration

### Daily Use (Default Mode)

```bash
# Use double-click app
# Or command line:
./target/release/imgquality-hevc auto \
  --explore --match-quality --compress --ultimate \
  --apple-compat --recursive \
  input_dir --output output_dir
```

### Storage Optimization (Strict Mode)

```bash
./target/release/imgquality-hevc auto \
  --no-allow-size-tolerance \
  --explore --match-quality --compress --ultimate \
  --apple-compat --recursive \
  input_dir --output output_dir
```

### Quick Test (No Ultimate)

```bash
./target/release/imgquality-hevc auto \
  --explore --match-quality --compress \
  --verbose \
  input_dir --output output_dir
```

---

## More Info

- **Full Docs**: `cat README_v7.8.3.md`
- **Examples**: `cat USAGE_EXAMPLES.md`
- **Changelog**: `cat CHANGELOG_v7.8.3.md`
- **Summary**: `cat SUMMARY.md`

---

## Summary

| Feature | Status | Note |
|---------|--------|------|
| Root Cause | ✅ Found | v7.8 hardcoded 1% tolerance |
| Solution | ✅ Implemented | Configurable tolerance switch |
| Default | ✅ Unchanged | Backward compatible with v7.8 |
| Control | ✅ Provided | --no-allow-size-tolerance |
| Docs | ✅ Completed | Full usage guide |

**Version**: v7.8.3
**Date**: 2026-01-29
**Compatibility**: Backward compatible with v7.8
**Breaking Changes**: None

---

🎊 **Done! You can now choose between Default Mode and Strict Mode as needed.**

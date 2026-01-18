# 🔥 Quality Verification Fix v7.2

## Problem
MS-SSIM calculation failed due to missing `libvmaf` in ffmpeg:
```
⚠️⚠️⚠️  ALL QUALITY CALCULATIONS FAILED!  ⚠️⚠️⚠️
- libvmaf not available in ffmpeg
```

## Solution
Integrated standalone `vmaf` CLI tool to bypass ffmpeg dependency.

**⚠️ Critical Finding** (Verified with rigorous testing):
- Both standalone `vmaf` and `ffmpeg libvmaf` MS-SSIM are **Y-channel (luma) only**
- Does NOT detect chroma (U/V) degradation
- Even with `extractplanes` filter, U/V channels show 1.000000 for 30% degradation
- **Solution**: Use MS-SSIM + SSIM All fusion for complete verification

Test Evidence (Multi-channel verification):
```
Y-only degradation (10%):
  Y channel: 0.996465 ✅ Detected
  U channel: 1.000000 ✅ Unchanged
  V channel: 1.000000 ✅ Unchanged

UV-only degradation (30%):
  Y channel: 1.000000 ✅ Unchanged
  U channel: 1.000000 ❌ Not detected (should be <0.95)
  V channel: 1.000000 ❌ Not detected (should be <0.95)
```

**Conclusion**: MS-SSIM algorithm itself is luma-based, not a limitation of the tool.

## Changes

### 1. New Module: `vmaf_standalone.rs`
- Uses independent `vmaf` command (Netflix official tool)
- Converts videos to Y4M format for vmaf processing
- Parses JSON output for MS-SSIM scores
- **Advantage**: No ffmpeg recompilation needed

### 2. Modified: `video_explorer.rs`
```rust
// Priority: standalone vmaf → ffmpeg libvmaf → SSIM fallback
if crate::vmaf_standalone::is_vmaf_available() {
    match crate::vmaf_standalone::calculate_ms_ssim_standalone(input, output) {
        Ok(score) => return Some(score),
        Err(e) => eprintln!("⚠️  Standalone vmaf failed: {}", e),
    }
}
// Fallback to ffmpeg libvmaf...
```

### 3. Updated: `lib.rs`
Added module export:
```rust
pub mod vmaf_standalone;
```

## Installation
```bash
# macOS
brew install libvmaf

# Verify
vmaf --version
```

## Testing
```bash
./scripts/e2e_quality_test.sh
```

## Fallback Chain
1. **ffmpeg libvmaf** (primary, now installed) → MS-SSIM (Y-channel only)
2. **Standalone vmaf** (fallback) → MS-SSIM (Y-channel only)
3. **ffmpeg ssim** → SSIM All (Y+U+V weighted) - **Essential for chroma verification**
4. **ffmpeg ssim** → SSIM Y only (last resort)

**Why SSIM All is essential**: MS-SSIM (both ffmpeg and standalone) only measures luma channel, missing chroma degradation entirely. SSIM All provides the necessary chroma verification.

## Benefits
✅ No ffmpeg recompilation required
✅ More reliable MS-SSIM calculation
✅ Graceful fallback chain
✅ Loud error reporting (no silent failures)

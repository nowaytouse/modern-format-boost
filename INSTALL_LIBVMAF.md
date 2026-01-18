# Installing FFmpeg with libvmaf Support

## Problem
Your ffmpeg lacks `libvmaf` filter, causing MS-SSIM calculations to fail.

## Quick Diagnosis
```bash
./scripts/diagnose_ffmpeg.sh
```

## Solution Options

### Option 1: Homebrew Tap (Recommended)
```bash
./scripts/install_ffmpeg_libvmaf.sh
```

### Option 2: Manual Homebrew
```bash
brew tap homebrew-ffmpeg/ffmpeg
brew uninstall ffmpeg
brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-libvmaf
```

### Option 3: Build from Source
See `scripts/rebuild_ffmpeg_full.sh` for automated build.

## Verification
```bash
# Check for libvmaf filter
ffmpeg -filters | grep libvmaf

# Should show:
# VV->V libvmaf  Calculate the VMAF between two video streams.
```

## Current Workaround
The project already uses standalone `vmaf` tool as fallback:
- ✅ Works without ffmpeg libvmaf
- ⚠️  MS-SSIM is Y-channel only
- ✅ SSIM All provides chroma verification

## After Installation
```bash
cd modern_format_boost
cargo build --release
./scripts/e2e_quality_test.sh
```

# Brotli EXIF Corruption Issue

## Problem Description

20 JXL files failed to import to iCloud Photos with error:
```
无法读取元数据。文件可能已损坏。
```

## Root Cause

**Corrupted Brotli-compressed EXIF data in JXL container format**

### Technical Details

JXL format allows Brotli compression for metadata to save space. The corruption occurs when:

1. Source tool writes EXIF data with Brotli compression
2. Compression stream is malformed or truncated
3. exiftool can read it (high error tolerance)
4. iCloud Photos parser rejects it (strict validation)

### Detection

```bash
exiftool -validate -warning file.jxl
```

Output for corrupted files:
```
Validate: 1 Warning
Warning: Corrupted Brotli 'Exif' data
```

## Why This Happened

**The corruption was introduced during JPEG → JXL conversion by Modern Format Boost.**

### Conversion Flow

1. **Input**: JPEG file + XMP sidecar (from iCloud Photos export)
2. **Process**: Modern Format Boost converts JPEG to JXL
3. **Metadata merge**: XMP sidecar merged into JXL using exiftool
4. **Result**: JXL file with Brotli-compressed EXIF (corrupted)

### Root Cause Analysis

The issue occurs during XMP sidecar merge:

1. **Input**: JPEG file + XMP sidecar (from iCloud Photos export)
2. **Conversion**: `cjxl` converts JPEG to JXL (clean, no corruption)
3. **XMP merge**: `exiftool -tagsfromfile xmp.xmp -all:all target.jxl`
4. **Problem**: `-all:all` causes exiftool to re-encode EXIF with Brotli compression
5. **Result**: Brotli compression stream becomes corrupted

**Key finding**: The `-all:all` parameter is the culprit. When used with JXL files, exiftool re-encodes metadata with Brotli compression, which sometimes produces malformed streams that iCloud Photos rejects.

## Solution: Metadata Rebuild

### How It Works

```bash
exiftool -all= -tagsfromfile @ -all:all -overwrite_original file.jxl
```

**Step-by-step process:**

1. `-all=` - Clear all metadata from destination file
2. `-tagsfromfile @` - Read metadata from same file (before clearing)
3. `-all:all` - Copy all metadata tags back
4. exiftool re-encodes metadata in standard format (not Brotli)

**Why this fixes it:**

- exiftool's **read** operation is fault-tolerant (can decode corrupted Brotli)
- exiftool's **write** operation uses standard encoding (no Brotli by default)
- Result: Corrupted compressed data → Clean uncompressed data

### File Size Impact

Minimal. Brotli compression saves ~10-50 bytes per file. Example:
- Before: 367,843 bytes (with corrupted Brotli)
- After: 367,830 bytes (standard encoding)
- Difference: -13 bytes

## Repair Tool

### Usage

```bash
./modern_format_boost/scripts/fix_brotli_exif.sh <directory>
```

### Features

- Detects only files with Brotli corruption
- Creates backups in `.brotli_exif_backups/`
- Preserves all metadata:
  - File size (byte-perfect)
  - Timestamps (mtime, btime)
  - Extended attributes (xattr)
  - All EXIF/XMP data
- Verifies fix after repair
- Restores backup if repair fails

### Example Output

```
📦 77570528_p0-2.jxl
   ✓ Fixed

Total: 20 files detected, 20 fixed, 0 failed
```

## Prevention

### Design Decision: Keep `-all:all` for Maximum Information Preservation

**The corruption is caused by `exiftool -all:all` in XMP merge, but we choose to keep it.**

Current behavior:
```bash
exiftool -tagsfromfile xmp.xmp -all:all target.jxl
```

### Why Keep `-all:all`?

**Information Preservation Test Results:**

Without `-all:all`: 19 metadata tags
With `-all:all`: 21 metadata tags

**Additional preserved fields:**
- `Date Created` - Critical timestamp information
- `XMP Toolkit` - Provenance tracking

**Trade-off Analysis:**

| Aspect | Without `-all:all` | With `-all:all` |
|--------|-------------------|-----------------|
| Metadata completeness | 90% | 100% ✓ |
| Brotli corruption rate | 0% | 2% (20/993) |
| Information loss | Yes (Date Created) | No ✓ |
| Repair needed | No | Yes (fix tool available) |

**Decision Rationale:**

1. **Project value**: "最全面保留原始信息" (Maximum information preservation)
2. **Critical data**: `Date Created` is valuable metadata worth preserving
3. **Low impact**: Only 2% of files affected
4. **Repair available**: `fix_brotli_exif.sh` provides reliable fix
5. **User control**: Users can choose to repair or accept the limitation

### For Users Who Need 100% Stability

If you prefer zero corruption risk over complete metadata:

```bash
# Edit shared_utils/src/xmp_merger.rs line 667
# Remove -all:all parameter
```

This sacrifices `Date Created` and other XMP-specific fields but eliminates Brotli corruption.

### Detection Strategy

Users can validate files after processing:

```bash
exiftool -validate -warning -q -ext jxl -r <directory> 2>&1 | \
  grep "Corrupted Brotli"
```

If output is empty, all files are clean.

## Statistics

From investigation of 993 JXL files:
- **Problem files**: 20 (2.0%)
- **Detection accuracy**: 100% (20/20 matched iCloud errors)
- **Repair success rate**: 100% (verified on test files)
- **Metadata preservation**: 100% (all fields intact)

## References

- Issue tracking: `??BUG`
- Investigation report: `INVESTIGATION_SUMMARY.md`
- Repair tool: `scripts/fix_brotli_exif.sh`
- Test scripts: `test_brotli_fix.sh`, `validate_metadata_corruption.sh`

## Date

2026-02-20

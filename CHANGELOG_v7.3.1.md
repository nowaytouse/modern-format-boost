# v7.3.1 - Critical Directory Structure Fixes

## 🐛 Fixed Bugs

### 1. Directory Structure Lost in Fallback Scenarios
**Files Fixed:**
- `imgquality_hevc/src/main.rs` (line 901-920)
- `imgquality_av1/src/conversion_api.rs`
- `vidquality_av1/src/conversion_api.rs`
- `vidquality_hevc/src/conversion_api.rs` (4 locations)

**Problem:**
When files failed conversion or were skipped, the fallback copy logic used only `file_name`, losing directory structure.

**Example:**
```
Input:  all/1/参考/内容 猎奇/file.gif
Output: all_optimized/file.gif  ❌ (root directory)
Should: all_optimized/1/参考/内容 猎奇/file.gif  ✅
```

**Solution:**
```rust
let dest = if let Some(ref base_dir) = config.base_dir {
    let rel_path = input.strip_prefix(base_dir).unwrap_or(input);
    let dest_path = out_dir.join(rel_path);
    if let Some(parent) = dest_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    dest_path
} else {
    let file_name = input.file_name().unwrap_or_default();
    out_dir.join(file_name)
};
```

### 2. Progress Bar Output Mixing (Display Issue)
**Status:** Identified, not critical

**Problem:**
Parallel threads' progress bars interfere with each other, causing messages like "video stream +423.8 KB" to appear on JPG files.

**Analysis:**
- This is a **display-only issue**, not affecting actual conversion
- The JPG file itself is processed correctly
- The "video stream" message comes from another parallel thread processing an animated file

**Impact:** Low (cosmetic only)

## ✅ Test Results

```bash
$ bash scripts/test_directory_structure_v7.3.sh
✅ beach.png: Directory structure preserved
✅ broken.png: Failed file copied with directory structure  
✅ cat.gif: File converted/copied with directory structure
✅ All tests passed!
```

## 📊 Coverage

**Fixed Scenarios:**
- ✅ Conversion failures (broken files)
- ✅ Skip due to size increase
- ✅ Skip due to quality issues
- ✅ Skip due to modern format (avoid generation loss)
- ✅ Skip due to short duration (<3s)
- ✅ All converters (imgquality-hevc, imgquality-av1, vidquality-hevc, vidquality-av1)

**Preserved:**
- ✅ Full directory structure
- ✅ File timestamps
- ✅ XMP metadata (auto-merged)

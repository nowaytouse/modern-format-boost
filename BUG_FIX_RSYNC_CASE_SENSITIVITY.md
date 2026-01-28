# Bug Fix Report: Uppercase File Duplication in Adjacent Output Mode

## Issue Description
When using "Output to Adjacent Folder" mode, files with uppercase extensions (e.g., `.JPG`, `.PNG`) were being incorrectly copied to the output directory as "non-media" files. This resulted in the output directory containing both the optimized version (e.g., `.jxl`) and the original unprocessed file (e.g., `.JPG`).

## Root Cause
The `scripts/drag_and_drop_processor.sh` script uses `rsync` to copy non-media files to the output directory. The exclusion list used to prevent media files from being copied was using case-sensitive patterns (e.g., `*.jpg`). Since `rsync` (on macOS/Unix) treats patterns case-sensitively, `*.jpg` did not match `*.JPG`.

## The Fix
Updated the exclusion patterns in `scripts/drag_and_drop_processor.sh` to use case-insensitive bracket expressions.

**Before:**
```bash
--exclude="*.jpg" --exclude="*.jpeg" ...
```

**After:**
```bash
--exclude="*.[jJ][pP][gG]" --exclude="*.[jJ][pP][eE][gG]" ...
```

This ensures that all variations (lowercase, uppercase, mixed case) are correctly identified as media files and excluded from the "non-media" copy step.

## Verification
- **Test Case:** Created a test directory with `test.JPG`.
- **Pre-fix:** `rsync --exclude="*.jpg"` copied `test.JPG` to destination.
- **Post-fix:** `rsync --exclude="*.[jJ][pP][gG]"` correctly skipped `test.JPG`.

## Impact
- **No Data Loss:** This change only affects which files are *excluded* from the "others" copy step. Media files are still processed by the optimization tools.
- **Cleaner Output:** Users will no longer see duplicate original files in the output folder for uppercase extensions.

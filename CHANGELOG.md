# Changelog

All notable changes to this project will be documented in this file.

## [8.2.0] - 2026-02-20

### 🍎 Apple Ecosystem Compatibility (Critical Fixes)
- **"Unknown Error" Resolved**: Fixed a critical issue where Apple Photos refused to import files due to extension mismatch (e.g., WebP files renamed as .jpeg).
- **WebP Disguised as JPEG**: Implemented `Magic Bytes` detection. The tool now ignores the literal file extension and inspects the file header. If a `.jpeg` is actually a WebP, it automatically routes it through `dwebp` pre-processing to ensure a valid JXL output.
- **Corrupted JPEG Repair**: Added pre-processing for JPEGs with illegal headers (e.g., missing `FF D8` start bytes). These are now sanitized using ImageMagick before conversion, preventing decoder crashes.
- **Nuclear Metadata Rebuild**: When `Apple Compatibility` mode is enabled, the tool now performs a "Nuclear Rebuild" (`exiftool -all=`) on metadata. This strips out "toxic" non-standard tags injected by third-party editors (e.g., Meitu) that cause Apple Photos to reject valid files.
- **Directory Timestamp Preservation**: Fixed an issue where processing files would update the parent directory's modification time. The tool now recursively saves and restores timestamps for all affected directories (deepest-first).

### ⚡ Core Improvements
- **Smart Format Detection**: Moved away from trusting file extensions. The core logic now relies on binary signatures for `jpg`, `png`, `gif`, `tif`, `webp`, and `mov`.
- **Robust Pre-processing**: Integrated `magick` and `dwebp` deeply into the Rust pipeline to handle edge cases that previously caused `cjxl` to fail.

### 🎨 UI/UX
- **Enhanced Logging**: Redesigned the CLI output with hierarchical styling.
  - **Important Alerts**: Now displayed in **Bold/Colored** text.
  - **Technical Details**: Now displayed in **Dimmed (Gray)** text to reduce visual noise.
- **Status Indicators**: Added clearer emojis (`✅`, `⚠️`, `🔧`) for operation states.

## [8.1.0] - 2026-02-15
- Initial release of the `modern_format_boost` Rust rewrites.

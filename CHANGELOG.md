# Changelog

All notable changes to this project will be documented in this file.

## [8.5.2] - 2026-02-23

### 🎬 MS-SSIM 极限模式时长参数

- **极限模式（--ultimate）**：MS-SSIM 跳过阈值由 5 分钟改为 **25 分钟**；仅当视频 >25 分钟时才跳过 MS-SSIM、仅用 SSIM 验证。
- **实现**：`gpu_coarse_search`、`video_explorer.validate_quality` 在 ultimate 下使用 25 min 阈值；`ssim_calculator.calculate_ms_ssim_yuv` 新增参数 `max_duration_min`（5.0 或 25.0），日志中显示对应阈值（如「≤25min」/「>25min」）。
- **文档**：CODE_AUDIT.md 新增 34 节「极限模式下 MS-SSIM 跳过阈值延长（25 分钟）」。

## [8.5.1] - 2026-02-23

### 📋 Audit follow-up (文档与可见性)

#### 算法与设计文档
- **Phase 2 搜索**（`video_explorer.rs`）：补充注释——CRF–SSIM 单调性假设；为何采用单点黄金比例而非完整黄金分割搜索（实现简单、每轮同样 1 次编码，仅可能多 1～2 次编码）。
- **迭代上限**（`video_explorer.rs`）：为长视频/超长视频的迭代上限常量添加文档，说明「更长视频 → 更低迭代上限」为有意为之的成本/精度权衡。
- **效率因子**（`quality_matcher.rs`）：模块与 `efficiency_factor()` 的文档中注明 H.264/HEVC/AV1 等为经验相对效率，可参考编解码比较研究，无单一权威引用。

#### 质量验证可见性
- **长视频跳过 MS-SSIM**：在 `ssim_calculator.rs`、`gpu_coarse_search.rs`、`video_explorer.rs`、`msssim_sampling.rs` 四处，将「跳过 MS-SSIM」的日志统一为带 ⚠️ 的警告级表述（"Quality verification: … MS-SSIM skipped"），便于用户知晓质量验证降级为仅 SSIM。

#### 审计文档
- **CODE_AUDIT.md**：新增「为何不用完整黄金分割搜索」说明；与代码注释一致。

## [8.5.0] - 2026-02-23

### 📋 Logging & Concurrency

#### Per-file log context (fix interleaved output)
- **Thread-local log prefix**: When processing multiple files in parallel, every `log_eprintln!` / `verbose_eprintln!` line is prefixed with `[filename]` so output can be attributed to the correct file.
- **Set at entry points**: `vid_hevc` `auto_convert()` and `img_hevc` `auto_convert_single_file()` set the prefix from the input file name and clear it on drop via `LogContextGuard`.
- **XMP distinct**: XMP merge milestone lines use a fixed `[XMP]` prefix so they are clearly separate from file-tagged lines.

#### Formatted indentation
- **Fixed-width tag column** (`LOG_TAG_WIDTH = 34`): All message bodies align so `[file.jpeg]`, `[file.webp]`, and `[XMP]` lines start the message at the same column.
- **Padding**: `pad_tag()` pads the tag so SSIM/CRF/XMP lines are visually aligned and easier to scan.

#### UTF-8 safe prefix
- **No panic on CJK filenames**: Prefix truncation now uses `truncate_to_char_boundary()` so we never slice through a multi-byte character (e.g. Chinese/Japanese in file names).
- **Shorter default**: `LOG_PREFIX_MAX_LEN` reduced to 28 to reduce log noise.

### ⏱️ Duration detection

#### ImageMagick fallback for WebP/GIF
- **Problem**: Animated WebP (and some GIF) often have no `stream.duration`, `format.duration`, or usable `frame_count`/fps from ffprobe, causing "DURATION DETECTION FAILED" and conversion to abort.
- **Solution**: In `detect_duration_comprehensive()` (precheck), after all ffprobe-based methods fail, try ImageMagick: `get_animation_duration_and_frames_imagemagick(path)` using `identify -format "%T"` to get (duration_secs, frame_count), then infer fps and return `(duration, fps, frame_count, "imagemagick")`.
- **API**: `image_analyzer::get_animation_duration_and_frames_imagemagick(path)` returns `Option<(f64, u64)>` without logging; existing `try_imagemagick_identify` uses it and keeps the "WebP/GIF animation detected" log.

### 🎬 GIF / animated quality verification

#### QualityCheck message when verification skipped
- When GIF input uses the size-only path (SSIM-All verification failed or unavailable), the summary line is now **"QualityCheck: N/A (GIF/size-only, quality not measured)"** instead of "FAILED (quality not verified)", so batch logs are less alarming and reflect expected behavior.

#### Real quality verification for GIF (and transparent inputs)
- **Direct + format normalization**: `calculate_ssim_all()` now tries (1) direct `[0:v][1:v]ssim`, (2) format normalization: both streams to `yuv420p` and even dimensions so GIF palette and HEVC output are comparable.
- **Alpha flatten (transparent GIF/WebP/PNG)**: Third fallback matches the encoder: input is converted with `format=rgba,premultiply=inplace=1,format=rgb24,format=yuv420p` (composite on black) then compared to HEVC output, so transparent pixels are evaluated on the same basis as the encoded file.
- **Helper**: `run_ssim_all_filter(input, output, lavfi)` runs a given lavfi graph and parses SSIM Y/U/V/All from stderr with validity checks.

### 🛠️ Technical

- **progress_mode** (`shared_utils`): `set_log_context`, `clear_log_context`, `format_log_line`, `LogContextGuard`, `pad_tag`, UTF-8-safe `set_log_context`.
- **precheck** (`video_explorer`): ImageMagick duration fallback after stream/format/frame_count+fps.
- **stream_analysis** (`video_explorer`): `calculate_ssim_all` multi-step fallback (direct → format_norm → alpha_flatten); `run_ssim_all_filter` for reusable lavfi + parse.
- **gpu_coarse_search** (`video_explorer`): `quality_verification_skipped_for_format` flag for GIF and friendlier QualityCheck line.

## [8.2.2] - 2026-02-20

### 🔥 Critical Bug Fixes

#### WebP/GIF Animation Duration Detection
- **Fixed ffprobe N/A Issue**: ffprobe returns `N/A` for WebP/GIF animation duration metadata
- **Added ImageMagick Identify Fallback**: New detection method using `identify -format "%T"` to read frame delays in centiseconds
- **Accurate Duration Calculation**: Sums all frame delays to calculate total animation duration
- **Impact**: 35+ animated WebP files that were previously skipped will now be correctly converted:
  - Duration ≥3s → HEVC MP4
  - Duration <3s → GIF (Bayer 256 colors)

#### Extension Mismatch Handling
- **Content-Aware Extension Correction**: Files are now renamed to match their actual content format before processing
  - `.jpeg` containing HEIC → renamed to `.heic`
  - `.jpeg` containing WebP → renamed to `.webp`
  - `.jpeg` containing PNG → renamed to `.png`
  - `.jpeg` containing TIFF → renamed to `.tiff`
- **Prevents Wrong Re-encoding**: Fixed issue where HEIC/WebP files with `.jpeg` extension were incorrectly re-encoded as JPEG by ImageMagick structural repair

#### On-Demand Structural Repair
- **Changed from Unconditional to On-Demand**: ImageMagick structural repair now only runs when exiftool detects metadata corruption
- **Performance Improvement**: Saves 100-300ms per file for healthy files (no unnecessary re-encoding)
- **Quality Protection**: Avoids unnecessary re-encoding for files without metadata issues

### 🌐 Internationalization

#### Complete English Output
- **All User-Facing Messages**: Converted from Simplified Chinese to English
- **Error Messages**: Full English translations for all error outputs
- **Console Output**: All processing logs, warnings, and success messages now in English
- **Comments**: Code comments translated to English for better maintainability

### 📦 Dependencies Updated
- `console`: 0.15 → 0.16
- `tempfile`: 3.10 → 3.20
- `proptest`: 1.4 → 1.7

### 🛠️ Technical Improvements
- **Magic Bytes Detection**: Extended to support HEIC brands (heic, heix, heim, heis, mif1, msf1)
- **Smart File Copier**: New module for content-aware extension correction
- **Improved Error Handling**: Better fallback mechanisms for format detection failures

## [8.2.1] - 2026-02-20

### 🔧 UI Text Fixes
- **Menu Option Renamed**: "Brotli EXIF Fix Only" → "Fix iCloud Import Errors"
- **Clearer Description**: "Fix corrupted Brotli EXIF metadata that prevents iCloud Photos import"

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

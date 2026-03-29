# Modern Format Boost v0.11.1 - Efficiency & Hardening

This release focuses on industrial-grade hardening, advanced error architecture, and significant efficiency optimizations for both image and video processing pipelines.

### 🛡️ Unified Error Architecture

- **Strict Classification**: Re-engineered `UnifiedError` to distinguish between **Fatal**, **Recoverable**, and **Optional** (skip) states.
- **Smart Skip Logic**: Non-productive conversions (output >= input) are now correctly categorized as **Optional (⏭️)**, ensuring cleaner reports and logs.
- **Anomaly Tracking**: Contextual capturing of upstream data inconsistencies (e.g., `ffprobe` anomalies) for faster troubleshooting.

### ⚡ Performance & Efficiency Hardening

- **Video Search Gate**: Mandatory safety gate for long-duration videos (>20 min) that intelligently skips expensive `CRF 0.00` probes if high-quality candidates already meet size requirements.
- **GIF "Lossless-First"**: Specialized reverse-exploration search for GIF-to-video conversion, achieving 1-pass success for ~90% of cases.
- **Search Sprint Optimization**: Halves search steps near boundaries to prevent overshoot and ensures a guaranteed floor check at `CRF 0.00` in Phase 4.

### 🔍 Precision Image Hardening

- **JPEG EOI Verification**: Implemented `is_jpeg_complete` to detect missing End-of-Image markers (`FF D9`) and skip expensive transcoding of truncated files.
- **UltraHDR Preservation**: Hardened detection for XMP gainmaps and MPF segments to ensure high-fidelity preservation of original assets.
- **APNG Optimization**: Fixed redundant animation probes for static PNGs with stray animation chunks.

### 🌍 Global Standardization & Quality

- **English-Only Transition**: Completed the project-wide migration to strictly English terminal messages and logs across all libraries and tools.
- **Magic Bytes Standard**: Standardized magic byte verification (e.g., `GIF8`) for deterministic format detection independent of file extensions.
- **Kondo Integration**: Integrated surgical repo cleanup directly into the `check_all.py` quality scanner for a leaner workspace.
- **Zero-Warning Production**: Maintained a 100% clean sweep across all Clippy categories (standard/pedantic/nursery) for guaranteed stability.

---

_For full details, please refer to the [CHANGELOG.md](CHANGELOG.md)._

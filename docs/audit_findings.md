# Modern Format Boost — Audit Findings

> Generated: 2026-04-17

This document catalogues issues found during a full codebase audit.
Items are grouped by severity and subsystem.

---

## Critical (Fixed)

### 1. ProRes HEVC Single-Thread Bottleneck
**Files:** `x265_params.rs`, `constants.rs`
**Status:** ✅ Fixed

`X265MemoryProfile::LowMemory` forced `frame-threads=1` for ALL ProRes files
(any size) and any file ≥8GB, wasting 75-90% of CPU. Replaced with a 3-tier
RAM-aware system (`Default`/`Moderate`/`LowMemory`) that queries actual
available system RAM via `system_memory::get_available_memory_mb()`.

---

## High Priority

### 2. `memory_profile_for_source()` Receives `None` Codec in 3 Call Sites
**Files:**
- `video_explorer.rs:2540` — `memory_profile_for_source(None, self.input_size)`
- `video_explorer.rs:2609` — `memory_profile_for_source(None, self.input_size)`
- `explore_strategy.rs:513` — `memory_profile_for_source(None, self.input_size)`

**Impact:** The ProRes/DNxHD codec signal is lost, so the profile decision
relies only on file size (≥8GB threshold). A 5GB ProRes file would get `Default`
profile even though it warrants care. These should pass the actual codec name
from probe/detection data.

### 3. GPU Calibration File Size Bug
**File:** `dynamic_mapping.rs:175-189`

The GPU calibration test encodes to `/dev/null` via `-f null` output but then
checks `fs::metadata(&gpu_path).map_or(0, |m| m.len())`. Since no file is
written to `gpu_path`, `gpu_size` is always 0 and calibration always fails on
the first attempt. The code works around this by trying multiple CRF values,
but the first iteration is always wasted.

---

## Medium Priority

### 4. Animated Image CRF 0 (Lossless) Overuse
**File:** `animated_image.rs:676`

`convert_to_mp4()` uses CRF 0 for ALL animated→video conversions. For GIF
stickers and memes, this produces unnecessarily large lossless HEVC files.
A CRF of 18-20 would be more appropriate for typical animated content, with
CRF 0 reserved for high-quality animated sources.

### 5. WebP Variable-Delay Frame Timing
**File:** `animated_image.rs:207-213`

`extract_webp_to_apng()` parses duration from the first frame only and assumes
all frames share the same duration. Variable-delay WebP animations (common in
chat stickers) get incorrect timing in the intermediate APNG, which propagates
to the final video output.

### 6. `gpu_coarse_search.rs` Monolith Function
**File:** `gpu_coarse_search.rs` (4839 lines total)

`cpu_fine_tune_from_gpu_boundary()` is ~2500 lines with inline comments
acknowledging "intentionally large". This makes the function very difficult to
maintain, test, or debug. Should be decomposed into per-phase functions.

---

## Low Priority

### 7. CRF Cache Unbounded Allocation
**File:** `explore_strategy.rs:127-134`

`CrfCache` allocates a fixed `Box<[Option<T>; 6400]>` array regardless of
actual usage. For short encodes testing 3-5 CRF values, this wastes memory.
Consider using a `HashMap` or smaller array with dynamic growth.

### 8. Spinner Thread Title Overflow
**File:** `drag_and_drop_processor.py:176-189`

The elapsed-time spinner writes to terminal title every 150ms. On very long
runs (days+), the title format string grows but has no upper bound on the
formatted width. Minor cosmetic issue.

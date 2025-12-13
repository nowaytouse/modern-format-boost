# Code Audit Checklist v5.1

## 1. Core Logic & Flags
### A. Flag Validation (`shared_utils/src/flag_validator.rs`)
- [x] **Logic Check**: `validate_flags` correctly handles combinations of `explore`, `match_quality`, `compress`.
    - `explore` only -> `FlagMode::ExploreOnly` (SizeOnly)
    - `match_quality` only -> `FlagMode::QualityOnly` (QualityMatch)
    - `compress` only -> `FlagMode::CompressOnly`
    - `explore` + `match_quality` -> `FlagMode::PreciseQuality`
    - `explore` + `match_quality` + `compress` -> `FlagMode::PreciseQualityWithCompress`
    - `compress` + `match_quality` -> `FlagMode::CompressWithQuality`
    - Invalid: `explore` + `compress` (without match_quality) is rejected.
- [x] **Implementation Usage**:
    - `imgquality_av1`: Uses `validate_flags_result` in `main.rs`.
    - `vidquality_hevc`: **ISSUE**: Does NOT explicitly call `validate_flags_result` in `main.rs`. Potential inconsistency.

### B. Video Explorer (`shared_utils/src/video_explorer.rs`)
- [x] **Modes Implementation**:
    - `SizeOnly`: Tests `max_crf`. If it compresses, returns it. Correct for "smallest file".
    - `QualityMatch`: Single pass using predicted CRF. Validates quality.
    - `CompressOnly`: Tests `initial_crf`. If fails, binary searches between `initial` and `max` for compression boundary.
    - `PreciseQualityMatch`: Binary searches (golden section + fine tune) for highest SSIM.
- [ ] **Potential Issue**: `explore_size_only` logic is very aggressive (jumps straight to `max_crf`). If `max_crf` looks bad, this mode doesn't care. (Working as designed, but worth noting).
- [x] **GPU Integration**: Uses `gpu_accel` crate. Supports force CPU via `use_gpu` flag.

### C. Quality Matcher (`shared_utils/src/quality_matcher.rs`)
- [x] **Algorithm**: Uses `effective_bpp` calculation based on:
    - Video bitrate (excluding audio)
    - GOP size & B-frames
    - Chroma subsampling & Color space
    - Codec efficiency factors (AV1 vs HEVC vs H264)
- [x] **"AI" Clarification**: The "AI prediction" described in user prompts appears to be a **heuristic algorithm** based on BPP and content analysis formulas, not a Neural Network.
    - **Formula**: `CRF = Base - Factor * log2(BPP * Scale)`

## 2. Tool-Specific Implementations
### A. `imgquality_av1`
- [x] **Command Structure**: `analyze`, `auto`, `verify`.
- [x] **Feature Parity**: Supports `explore`, `compress`, `match_quality` flags.
- [x] **Safety**: Checks dangerous directories.
- [x] **Concurrency**: Uses `rayon` for parallel processing.

### B. `vidquality_hevc`
- [x] **Command Structure**: `analyze`, `auto`, `simple`, `strategy`.
- [ ] **Flags**: `vidquality` implements manual config creation instead of shared validator.
- [x] **Features**: Implements recursive scanning, batch reporting.

## 3. General Architecture
- [x] **Modular Design**: `shared_utils` holds common logic (GPU, Flags, Explorer).
- [x] **GPU Detection**: Robust `GpuAccel` struct with vendor detection (Nvidia, Apple, Intel, AMD).
- [x] **Error Handling**: Uses `anyhow` result types.

## 4. Documentation Status
- [x] **Outdated**: `CODE_AUDIT_CHECKLIST_v5.0.md` (Missing/Old).
- [x] **New**: `CODE_AUDIT_CHECKLIST_v5.1.md` created.
- [x] **Action Item**: Need to update `README.md` to remove marketing "AI" jargon if it implies ML, replacing with "Smart Heuristic" or "Advanced Algorithm".

## 5. Critical Bugs / Edge Cases
- [x] **Explore Size Only**: Verified logic (max_crf strategy).
- [x] **FFmpeg Deadlock**: `video_explorer.rs` v5.2 update mentions "background thread for stderr" to prevent deadlock. Code review confirms `stderr_handle` spawns a thread. **FIXED**.

## 6. Recommendations
1. [x] Update `vidquality_hevc/src/main.rs` to use `shared_utils::validate_flags_result` for consistency.
2. [x] Clarify "AI" terminology in user-facing text.
3. [x] Verify `explore_size_only` behavior: ensure `max_crf` produces valid video (it might be too compressed). Consider adding a safety check or simple validation even for SizeOnly.
4. [ ] Add unit tests for `vidquality_hevc` flag combinations.

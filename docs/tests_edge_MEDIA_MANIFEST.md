# Edge Media Manifest

This directory contains synthetic test media assets for the
`modern_format_boost` development and integration test suite.

## Video Assets (videos/)

| Filename                     | Description                                      | Codec      | Duration |
| :--------------------------- | :----------------------------------------------- | :--------- | :------- |
| `test_h264_10s.mp4`          | Baseline H.264 video for CRF/SSIM testing        | H.264/AVC  | 10s      |
| `test_vp9_5s.webm`           | VP9 WebM with Opus audio; normal mode skip case  | VP9        | 5s       |
| `test_hevc_8s.mp4`           | HEVC loop candidate                              | HEVC/H.265 | 8s       |
| `test_av1_6s.mkv`            | AV1 container compatibility test                 | AV1        | 6s       |
| `test_hq_source_15s.mp4`     | High-quality source for threshold verification   | H.264/AVC  | 15s      |
| `test_lq_source_12s.mp4`     | Low-quality/noisy source for detector robustness | H.264/AVC  | 12s      |
| `test_short_2s.mp4`          | Short H.264 clip with audible AAC; HEVC route    | H.264/AVC  | 2s       |
| `test_definitively_long.mp4` | 18.5s clip to trigger DefinitivelyLong tier      | H.264/AVC  | 18.5s    |
| `non_monotonic.mp4`          | Poison pill with timestamp anomalies             | H.264/AVC  | ~5s      |

## GIF Assets (gifs/)

| Filename                         | Description                             | Type           |
| :------------------------------- | :-------------------------------------- | :------------- |
| `test_simple.gif`                | Basic looping GIF                       | Static Palette |
| `test_pattern.gif`               | Dynamic pattern GIF for motion analysis | Large Palette  |
| `simulated_headless_sticker.gif` | Headless sticker regression asset       | Transparent    |
| `non_monotonic.apng`             | APNG with non-monotonic timestamps      | Animation      |

## Quality Metric Requirements

- Supports **H.264** probing.
- Supports **AV1** container and stream parsing.
- Validates **CRF** search convergence.
- Validates **SSIM** quality grading.

---

### Created by Modern Format Boost CI/CD

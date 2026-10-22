# Local JPEG Reconstruction Fix

This directory vendors only `jxl-jbr` 0.2.1 from
[`tirr-c/jxl-oxide`](https://github.com/tirr-c/jxl-oxide), commit
`c0cc4c7ea57c1207f38ff2970d94757470613be4`.
The upstream MIT and Apache-2.0 licenses are included without modification.

## Changes

- `Cargo.toml` is normalized for use outside the upstream workspace. Its sibling
  crates retain the upstream Git source; the repository's `Cargo.lock` fixes their
  revisions. No new codec implementation or dependency is introduced.
- `src/reconstruct.rs` derives scan sampling denominators from every image color
  component, rather than only the components selected by the current scan. A
  non-interleaved chroma scan in a subsampled JPEG must retain the full image's
  sampling denominator. Using the full-resolution scan width can otherwise read
  past the decoded coefficient grid.
- `src/reconstruct/scan.rs` reports an invalid scan coefficient range as an error
  instead of passing it to a panicking subgrid accessor. No samples are clamped,
  discarded, or substituted.

The scan geometry follows `JPEGData::CalculateMcuSize` in the existing libjxl
source (`lib/jxl/jpeg/jpeg_data.cc`): for a non-interleaved scan, the number of
MCUs is `ceil(dimension * component_sampling / (8 * maximum_image_sampling))`.
Interleaved scans use one MCU per maximum-image sampling unit.

This patch preserves independent full-byte JPEG reconstruction verification.
Remove the local override only after an upstream fix passes the same progressive
subsampling, complete-byte restoration, and invalid-input regression tests.

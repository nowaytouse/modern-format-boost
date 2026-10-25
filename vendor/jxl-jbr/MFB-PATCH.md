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
- `src/reconstruct.rs` accepts the adaptive LF smoothing rendering flag without
  changing JPEG reconstruction. Reconstruction reads raw integer `lf_quant`
  channels from `LfGroup`; the renderer copies/dequantizes those channels into a
  separate floating-point image before smoothing it. External LF-frame
  references (`USE_LF_FRAME`) remain rejected; all other frame/quantization checks
  remain in force. Complete reconstructed JPEG bytes still require independent hashing.

The scan geometry follows `JPEGData::CalculateMcuSize` in the existing libjxl
source (`lib/jxl/jpeg/jpeg_data.cc`): for a non-interleaved scan, the number of
MCUs is `ceil(dimension * component_sampling / (8 * maximum_image_sampling))`.
Interleaved scans use one MCU per maximum-image sampling unit.

The LF distinction follows the pinned upstream `jxl-frame/src/data/lf_group.rs`,
`jxl-vardct/src/lf.rs`, `jxl-render/src/util.rs` and
`jxl-render/src/vardct/mod.rs`: LF coefficient parsing does not apply the rendering
filter. This is not permission to reconstruct JPEGs from smoothed display pixels
or to accept a byte mismatch.

## Synthetic LF Smoothing Regression

`crates/foundation/tests/fixtures/synthetic_jpeg_lfs1.{jpg,jxl}.fixture` contains
only a generated 64-by-48 color gradient, with profiles/comments stripped. It was
created using ImageMagick 7.1.2-35 Q16-HDRI (`a0c4bf729`) and official cjxl/djxl
0.13.0 (`777e88f4`):

```sh
magick -size 64x48 'gradient:#112244-#eecc88' -depth 8 \
  -sampling-factor 1x1 -interlace none -quality 90 -strip source.jpg
cjxl source.jpg archive.jxl --lossless_jpeg=1 --effort=7 \
  --num_threads=1 --jpeg_reconstruction_lfs=1
djxl --reconstruct_jpeg --num_threads=1 archive.jxl reconstructed.jpg
cmp source.jpg reconstructed.jpg
```

The official reconstruction compared equal. The JPEG SHA-256 is
`55bdb8445e44d5e7f3545576da73a6850de5f390c9bdb43607f7d2465cfa2a99`;
the JXL SHA-256 is
`c342f3579c52a42d6aa78566ab1ca87aae99c97be1b140d644db13a23aacfbb7`.
The library regression requires the real frame's smoothing flag to be enabled,
checks complete independent reconstruction against that synthetic JPEG, and
rejects a JPEG with appended bytes. The fixed fixture keeps this regression
independent of the CI encoder's support for the newer command-line flag.

This patch preserves independent full-byte JPEG reconstruction verification.
Remove the local override only after an upstream fix passes the same progressive
subsampling, LF smoothing, complete-byte restoration, and invalid-input regression
tests.

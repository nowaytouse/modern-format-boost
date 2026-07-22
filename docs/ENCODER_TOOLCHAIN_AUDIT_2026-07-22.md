# Encoder Toolchain Audit - 2026-07-22

This audit records the authoritative tools and local precision measurements used by
Modern Format Boost. It is evidence for tool-selection defaults, not a claim that
one encoder setting wins on every image.

## Authority And Installed Versions

| Format         | Preferred tool                          | Installed build                              | Policy                                                                                                                          |
| -------------- | --------------------------------------- | -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| AVIF           | `avifenc` / `avifdec` from libavif      | 1.4.2, AOM 3.14.1, dav1d 1.5.4               | Use before FFmpeg for supported static inputs and frame sequences. Meme Mode explicitly uses speed 0.                           |
| JPEG XL        | `cjxl` / `djxl` / `jxlinfo` from libjxl | 0.13.0 HEAD-196a43d                          | Use first for JXL encode, decode, and structural inspection. Expert effort 11 requires an explicit expert flag.                 |
| HEIC/HEIF      | `heif-convert` from libheif             | 1.23.0 runtime tool                          | Use to normalize HEIC/HEIF sources when the target encoder cannot read the container directly.                                  |
| WebP           | `cwebp` / `dwebp` from libwebp          | 1.6.0                                        | Use before generic conversion tools.                                                                                            |
| Video/fallback | FFmpeg                                  | 8.1.2, custom `homebrew-ffmpeg/ffmpeg` build | Preserve the custom feature-complete installation; use it for video and when no authoritative format tool covers the operation. |

Authoritative references:

- [libavif repository](https://github.com/AOMediaCodec/libavif)
- [avifenc manual](https://github.com/AOMediaCodec/libavif/blob/main/doc/avifenc.1.md)
- [libjxl repository](https://github.com/libjxl/libjxl)
- [FFmpeg official downloads](https://ffmpeg.org/download.html)

The avifenc manual defines speed 0 as the slowest setting and speed 10 as the
fastest. It also documents JPEG, PNG, Y4M, and image-sequence inputs. Inputs such
as AVIF, HEIC/HEIF, and WebP therefore pass through their official decoder once
before avifenc, while the original file remains the delivery and verification
source.

## Reproducible Precision Samples

The source was generated directly as a true, non-JPEG PNG and validated with
`pngcheck`. The 768 x 768 source was 16-bit RGBA, non-interlaced, 3,457,493 bytes,
with SHA-256:

```text
0639e72cdb3d122921bd71dcdc5ccb99d5e7ecabd7fb41a8046257563bf0bf1e
```

### JPEG XL effort 10 versus expert effort 11

The completed comparison used a 256 x 256 true PNG source of 374,587 bytes.

| Setting                   |     Time |    Output | Pixel proof               |
| ------------------------- | -------: | --------: | ------------------------- |
| lossless effort 10        |   6.30 s | 292,013 B | exact, ImageMagick AE = 0 |
| lossless expert effort 11 | 229.06 s | 280,514 B | exact, ImageMagick AE = 0 |

On this source, effort 11 was about 36.4 times slower for about 3.94 percent less
output than effort 10. A 768 x 768 effort-11 run was stopped after more than three
minutes, while effort 10 completed in 26.87 seconds. Production lossless PNG uses
effort 10; effort 11 remains an explicit expert-only choice.

### AVIF speed 0 versus speed 6

Both runs used quality 95, alpha quality 95, YUV444, and CICP 1/13/0 on the same
768 x 768 true PNG.

| Setting |    Time |      Output |
| ------- | ------: | ----------: |
| speed 0 | 14.93 s | 1,029,777 B |
| speed 6 |  0.44 s | 1,029,474 B |

Speed 0 was about 34 times slower and was slightly larger on this sample. This
does not justify silently relaxing Meme Mode: its contract prioritizes maximum
encoder search precision, so it keeps speed 0. The measurement is retained to
make the performance cost visible and to prevent unsupported claims that speed 0
always produces a smaller file.

## Operational Rule

Use the authoritative format implementation when it supports the requested
operation. Use a current development build when it is locally validated and adds
needed capability. Fall back to FFmpeg or another generic tool only after the
format-native route is unavailable or has failed with a classified reason. Every
fallback must preserve the original as the verification source and must pass the
same metadata, decode, orientation, and size gates.

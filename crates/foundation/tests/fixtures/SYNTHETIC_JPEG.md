# Synthetic JPEG Reconstruction Fixtures

The three `synthetic_progressive_*.jpg.fixture` files contain only a generated
19-by-21 solid-color image (`#4a7fbc`), not user media or external image content.

They were created with libjpeg-turbo `cjpeg` 3.2.1, quality 90, progressive scans,
and the following explicit sampling factors:

| Fixture | Sampling factors |
| --- | --- |
| `synthetic_progressive_420.jpg.fixture` | `2x2,1x1,1x1` |
| `synthetic_progressive_422.jpg.fixture` | `2x1,1x1,1x1` |
| `synthetic_progressive_440.jpg.fixture` | `1x2,1x1,1x1` |

The non-aligned dimensions exercise partial edge MCUs. Tests encode each known
JPEG into reversible JXL using the existing native encoder, then require the
independent reconstruction to match every original JPEG byte. No external
encoder executable is required when running these tests.

The 4:2:0 fixture reproduces the pre-fix `jxl-jbr` coefficient-subgrid panic in
the production restoration preflight. The horizontal-only and vertical-only
subsampling fixtures cover both sampling denominators without private pixels.

# Third-Party Licenses

This project, **Modern Format Boost**, is built upon various open-source libraries. Below is a summary of the licenses governing these dependencies.

## 📜 Summary of Licenses

| License | Count | Description |
| :--- | :--- | :--- |
| **MIT / Apache-2.0** | ~160 | The majority of the Rust ecosystem (Dual-licensed). |
| **MIT** | 55 | Common permissive license. |
| **BSD-2-Clause / BSD-3-Clause** | 7 | Permissive licenses used by libraries like `rav1e`. |
| **MPL-2.0** | 1 | Mozilla Public License (used by `mp4parse`). |
| **Zlib** | 1 | Permissive license (used by `foldhash`). |
| **Unlicense** | 7 | Public domain-like (used by `walkdir`, `memchr`, etc.). |

---

## 📦 Detailed Dependency List

The following list was generated using `cargo-license`.

### Apache-2.0 OR MIT (Dual Licensed)
*Used by the vast majority of core dependencies including: anyhow, chrono, clap, image, rayon, serde, etc.*

### MIT
- **img-hevc**, **vid-hevc**, **shared_utils** (This Project)
- **built**, **console**, **indicatif**, **libheif-rs**, **tracing**, **which**, etc.

### BSD-2-Clause / BSD-3-Clause
- **rav1e** (AV1 Encoder)
- **av1-grain**
- **ravif**
- **exr**

### MPL-2.0 (Mozilla Public License 2.0)
- **mp4parse**: Used for parsing MP4/MOV containers.
  - *Note: MPL-2.0 is a file-level copyleft license. Changes to mp4parse itself must be disclosed, but the rest of this project remains under MIT.*

### Zlib
- **foldhash**
- **miniz_oxide** (via Apache-2.0/MIT/Zlib)

---

## ⚖️ Compliance

- **Permissive Use**: Most dependencies are under MIT, Apache 2.0, or BSD, which are highly permissive.
- **Copyleft**: We do not use any GPL/LGPL dependencies that would require this project to change its license, ensuring it remains compatible with the MIT license.
- **Attribution**: This document serves as attribution for the authors listed in the dependency tree.

*Generated on: 2026-03-08*

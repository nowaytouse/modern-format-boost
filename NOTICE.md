# License Notice

This project, **Modern Format Boost**, is a collection of tools and libraries. To ensure clarity and compliance, the project uses multiple licenses depending on the component.

## ⚖️ Summary of Licenses

- **Code (.rs, .sh, .toml)**: Licensed under the [MIT License](LICENSE_MIT).
- **Assets (Images, Icons)**: Licensed under the [Creative Commons Attribution 4.0 International](https://creativecommons.org/licenses/by/4.0/) (CC BY 4.0).
- **Dependencies**: Various licenses (see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)).

## 📦 Runtime Dependencies

Modern Format Boost relies on several external binary tools at runtime. These are NOT bundled with the source code and must be installed separately. Their respective licenses apply:

| Tool | Primary License | Purpose |
| :--- | :--- | :--- |
| **FFmpeg** | LGPL / GPL | Video transcoding and analysis. |
| **ExifTool** | GPL / Artistic License | Metadata extraction and repair. |
| **ImageMagick** | ImageMagick License (Apache-compatible) | Image processing and verification. |
| **libjxl** | BSD-3-Clause | JPEG XL encoding/decoding. |

## 🤝 Acknowledgements

Special thanks to the open-source communities behind:
- The **Rust** language and the ecosystems on `crates.io`.
- The **FFmpeg** project for industrial-grade video processing.

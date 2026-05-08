# Third Party Licenses

Modern Format Boost uses various open source dependencies. This document provides information about the licenses of these dependencies.

## License Summary

The project and its dependencies use the following licenses:

- **MIT License** - Most Rust crates
- **Apache-2.0** - Many Rust ecosystem crates
- **Apache-2.0 WITH LLVM-exception** - LLVM-related dependencies
- **BSD-2-Clause / BSD-3-Clause** - Various system libraries
- **MPL-2.0** - Mozilla Public License dependencies
- **GPL-3.0-or-later** - JPEG XL libraries (jpegxl-rs, jpegxl-sys)
- **LGPL-2.1-or-later** - Some system interface libraries
- **Zlib** - Compression libraries
- **ISC** - Internet Systems Consortium license
- **CC0-1.0** - Public domain equivalent
- **Unicode-3.0** - Unicode data
- **IJG** - Independent JPEG Group
- **NCSA** - University of Illinois/NCSA

## Important Notes

### GPL-3.0-or-later Dependencies

The following dependencies use GPL-3.0-or-later license:

- **jpegxl-rs** - Rust bindings for JPEG XL
- **jpegxl-sys** - Low-level JPEG XL system bindings

These are used for JPEG XL format support. If you distribute this software, you must comply with GPL-3.0-or-later terms for these components.

For complete license texts and detailed attribution, see:

- **docs/LICENSES.html** - Full HTML report with all license texts
- **docs/LICENSES.json** - Machine-readable JSON format
- **about.toml** - cargo-about configuration

## Generating License Reports

To regenerate license reports:

```bash
# Generate reports using the project's nightly toolchain
cargo +nightly about generate --workspace --all-features docs/licenses-template/licenses.hbs -o docs/LICENSES.html
cargo +nightly about generate --workspace --all-features --format json -o docs/LICENSES.json
```

## Compliance

This project complies with all license requirements of its dependencies. All accepted licenses are listed in `about.toml` and verified using `cargo-about` and `cargo-deny`.

For questions about licensing, please open an issue on the project repository.

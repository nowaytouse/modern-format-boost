# Licensing and distribution

The Modern Format Boost source code carries the project [MIT license](../LICENSE).
The dependency notices shipped with the macOS app and command-line archives are
[LICENSES.json](LICENSES.json) for the locked Cargo workspace graph and
[THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) for native sources used by the
default build. The app also shows these notices in its license viewer.

## Review a binary release

The default image and video builds enable the GPL-3.0-or-later
`jpegxl-rs` and `jpegxl-sys` bindings through `foundation/jpegxl-ffi`.
The high-precision features can use LGPL components, including `rug` and
`gmp-mpfr-sys`. Review the actual artifact, including Cargo features, linked
native libraries, and any code compiled from vendored sources. Separately
installed tools such as FFmpeg, ImageMagick, and ExifTool retain their own terms.

Before distributing a binary, verify its applicable redistribution terms and
make any required corresponding source and relinking materials available for
the exact build. Use the dependency notices as inputs to that review. A passing
Cargo license check confirms the configured license allowlist.

- [GPL-3.0](https://www.gnu.org/licenses/gpl-3.0.html)
- [LGPL-3.0](https://www.gnu.org/licenses/lgpl-3.0.html)
- [SQLite public-domain statement](https://www.sqlite.org/copyright.html)

## Refresh the workspace notices

Run from the repository root after updating `Cargo.lock`. Generate the raw report
outside the checkout because Cargo metadata contains absolute build-machine
paths. With cargo-about 0.9.3 and jq installed:

```sh
scratch=$(mktemp -d)
cargo about generate --workspace --locked --offline --fail --format json -o "$scratch/raw.json" &&
  jq '
    def package_fields: {name, version, source, repository};
    {
      crates: [.crates[] | {package: (.package | package_fields), license}]
        | sort_by(.package.name, .package.version, .package.source),
      overview: (.overview | sort_by(.id)),
      licenses: [.licenses[] | {id, name, text, used_by: [.used_by[] | {crate: (.crate | package_fields)}]
        | unique_by(.crate.name, .crate.version, .crate.source)}]
        | sort_by(.id, .text)
    }
  ' "$scratch/raw.json" > "$scratch/LICENSES.json" &&
  cp "$scratch/LICENSES.json" docs/LICENSES.json
```

The filtered report keeps license texts, selected licenses, package versions,
and upstream identifiers. Offline generation uses the cached crate sources and
may fall back to standard license text when a crate omits its license file.
Check upstream notices for those crates and native notices against the vendored
library sources when preparing a release.

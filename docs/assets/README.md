# Brand Assets

`mfb-logo.png` is the V2 MFB raccoon-and-photo logo, rendered from the
user-provided vector artwork preserved in `crates/gui/src-macos/icon.svg`.
The SVG is the source of truth; the PNG is its 1254 × 1254 documentation render.
Both READMEs display this image at 128 × 128 without changing its transparent
corners or the artwork's original padding.

The macOS resource `crates/gui/src-macos/icon.icns` is rendered from the same SVG at
16, 32, 128, 256 and 512 points, each with a 2x Retina representation. To update
the approved artwork, render each PNG size directly from the SVG with
`rsvg-convert --width <pixels> --height <pixels>`, assemble the
`.iconset` with `iconutil`, and refresh the app using `smart_build --all --gui`.
Keep the README image and app resource synchronized; do not modify the source
artwork as part of icon packaging.

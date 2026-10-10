# Brand Assets

`mfb-logo.png` is the approved first MFB media-card logo. It is the original
AI-generated artwork, including its content provenance metadata. The mascot and
alternate concepts are not replacements for the primary application icon.

The macOS resource `crates/gui/src-macos/icon.icns` is derived from this image at
16, 32, 128, 256 and 512 points, each with a 2x Retina representation. To update
the approved artwork, regenerate those PNG sizes with macOS `sips`, assemble the
`.iconset` with `iconutil`, and refresh the app using `smart_build --all --gui`.
Keep the README image and app resource synchronized; do not modify the source
artwork as part of icon packaging.

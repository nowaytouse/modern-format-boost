#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
gui_dir=$(cd "$script_dir/.." && pwd)
project_root=$(cd "$gui_dir/../.." && pwd)
bundle="$project_root/target/release/bundle/macos/Modern Format Boost.app"
expected_bundle="$project_root/target/release/bundle/macos/Modern Format Boost.app"

if [[ "$bundle" != "$expected_bundle" ]]; then
    echo "Refusing unexpected bundle path: $bundle" >&2
    exit 1
fi
arch=$(uname -m)
case "$arch" in
arm64 | x86_64) ;;
*)
    echo "Unsupported macOS architecture: $arch" >&2
    exit 1
    ;;
esac

rm -rf -- "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"

xcrun swiftc \
    -swift-version 5 \
    -O \
    -target "$arch-apple-macos13.0" \
    -framework AppKit \
    -framework CoreServices \
    "$script_dir/main.swift" \
    -o "$bundle/Contents/MacOS/Modern Format Boost"

cp "$script_dir/Info.plist" "$bundle/Contents/Info.plist"
cp "$script_dir/icon.icns" "$bundle/Contents/Resources/icon.icns"
ditto "$script_dir/Resources" "$bundle/Contents/Resources"
plutil -lint "$bundle/Contents/Info.plist" >/dev/null

for strings_file in "$bundle"/Contents/Resources/*.lproj/*.strings; do
    plutil -lint "$strings_file" >/dev/null
done

echo "$bundle"

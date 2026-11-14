"""Check the release version contract without building native artifacts."""

from pathlib import Path
import plistlib
import tomllib


ROOT = Path(__file__).resolve().parents[3]
VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
PACKAGES = {
    "foundation": ROOT / "crates/foundation/Cargo.toml",
    "img": ROOT / "crates/img/Cargo.toml",
    "vid": ROOT / "crates/vid/Cargo.toml",
    "dev": ROOT / "crates/dev/Cargo.toml",
}

for name, path in PACKAGES.items():
    package = tomllib.loads(path.read_text())["package"]
    assert package.get("version") == {"workspace": True}, f"{name} must inherit workspace version"

lock_packages = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
for name in PACKAGES:
    locked = [package for package in lock_packages if package["name"] == name]
    assert len(locked) == 1 and locked[0]["version"] == VERSION, f"{name} lock version differs from {VERSION}"

for path in (
    ROOT / "crates/gui/src-macos/Info.plist",
    ROOT / "crates/gui/src-macos/PhotosImportHelper-Info.plist",
):
    plist = plistlib.loads(path.read_bytes())
    for key in ("CFBundleShortVersionString", "CFBundleVersion"):
        assert plist.get(key) == "$(MFB_VERSION)", f"{path.name} {key} must be stamped by smart_build"

print(f"Version contract OK: {VERSION}")

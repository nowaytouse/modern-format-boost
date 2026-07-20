#!/usr/bin/env python3
"""Modern Format Boost - Dependency Installer

Installs all required dependencies for the project including:
- System dependencies (ffmpeg, imagemagick, postgresql, etc.)
- Rust toolchain and components
- Cargo utilities
- Python tools
- Node.js tools

Supports macOS (Homebrew) and Linux (apt).

FFmpeg Installation Notes:
--------------------------
By default, this script installs the standard Homebrew ffmpeg.

For advanced users who need full-featured FFmpeg with plugins like FDK-AAC,
Chromaprint, AI filters, etc., you can use the homebrew-ffmpeg tap.

The "Link Overwrite" Strategy:
-------------------------------
Homebrew's standard `chromaprint` package strictly depends on the formula named
`ffmpeg`. To satisfy this dependency while using the enhanced version, we install
both and use Homebrew's linking system to toggle the active version.

Step-by-Step Instructions:
--------------------------

1. Install Official FFmpeg (satisfies dependencies for chromaprint, etc.):
   brew install ffmpeg

2. Install Full-Featured Tap Version:
   brew tap homebrew-ffmpeg/ffmpeg
   brew install homebrew-ffmpeg/ffmpeg/ffmpeg \\
       --with-chromaprint \\
       --with-dvd \\
       --with-fdk-aac \\
       --with-game-music-emu \\
       --with-ggml \\
       --with-jack \\
       --with-jpeg-xl \\
       --with-libaribcaption \\
       --with-libmodplug \\
       --with-libopenmpt \\
       --with-libplacebo \\
       --with-librist \\
       --with-librsvg \\
       --with-libsoxr \\
       --with-libssh \\
       --with-tensorflow \\
       --with-tesseract \\
       --with-libvidstab \\
       --with-openal-soft \\
       --with-openapv \\
       --with-opencore-amr \\
       --with-openh264 \\
       --with-openjpeg \\
       --with-openvino \\
       --with-rav1e \\
       --with-rtmpdump \\
       --with-rubberband \\
       --with-two-lame \\
       --with-webp \\
       --with-whisper-cpp \\
       --with-xvid \\
       --with-zeromq \\
       --with-zimg \\
       --with-srt \\
       --with-libvmaf \\
       --with-libxml2 \\
       --with-libzvbi \\
       --with-aribb24 \\
       --with-libbluray \\
       --with-libbs2b \\
       --with-libcaca \\
       --with-libdvdnav \\
       --with-libdvdread \\
       --with-libgsm \\
       --with-openssl@3 \\
       --with-speex

   Note: --with-decklink and --with-libflite are excluded as they require
   manual SDKs or have current platform issues.

3. Toggle to Full-Featured Version:
   brew unlink ffmpeg
   brew link --overwrite homebrew-ffmpeg/ffmpeg/ffmpeg

4. (Optional) Create Alias for Standard Version:
   ln -sf $(brew --prefix)/opt/ffmpeg/bin/ffmpeg $(brew --prefix)/bin/ffmpeg-official

This script will detect and preserve existing ffmpeg installations to avoid conflicts.
"""

from mfb_ui_tokens import pick_symbol
import os
import platform
import shutil
import subprocess
import sys

GREEN = "\033[0;32m"
BLUE = "\033[0;34m"
YELLOW = "\033[1;33m"
RED = "\033[0;31m"
DIM = "\033[2m"
NC = "\033[0m"


def print_c(color, text, end="\n"):
    print(f"{color}{text}{NC}", end=end)


def command_exists(cmd):
    return shutil.which(cmd) is not None


def run_cmd(cmd, check=True, capture_output=False):
    return subprocess.run(
        cmd, shell=True, check=check, capture_output=capture_output, text=True
    )


def main():
    from pathlib import Path

    _scripts = Path(__file__).resolve().parent
    if str(_scripts) not in sys.path:
        sys.path.insert(0, str(_scripts))
    from mfb_entry_guard import guard_main  # noqa: E402

    guard_main("install_deps.py")
    print_c(
        BLUE,
        f"{pick_symbol('🚀', ('[LAUNCH]'))} Modern Format Boost - Dependency Installer v0.11.3",
    )
    print("--------------------------------------------------------")
    print_c(
        DIM,
        "💡 For advanced FFmpeg setup (FDK-AAC, AI filters, etc.), see script header.",
    )
    print("--------------------------------------------------------\n")

    OS_TYPE = platform.system().lower()

    if OS_TYPE == "darwin":
        print_c(YELLOW, f"{pick_symbol('🍎', ('[APPLE]'))} Detected macOS")
        if not command_exists("brew"):
            print("Installing Homebrew...")
            run_cmd(
                '/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"'
            )
        print("Updating Homebrew...")
        run_cmd("brew update")
        print("Checking and installing system dependencies via Homebrew...")

        deps = [
            "jpeg-xl",  # cjxl, djxl, jxlinfo
            "jpeginfo",  # JPEG structural validator
            "pngcheck",  # PNG chunk/CRC validator
            "exiftool",  # Metadata preservation
            "exiv2",  # Metadata inspection/validation
            "imagemagick",  # Image format conversion (magick)
            "webp",  # WebP support (dwebp, cwebp)
            "libavif",  # AVIF decoder/validator
            "libheif",  # HEIF/HEIC support
            "coreutils",  # GNU core utilities
            "node",  # Node.js for prettier/markdownlint
            "shellcheck",  # Shell script linting
            "shfmt",  # Shell script formatting
            "postgresql@14",  # Database for ML training
            "pgvector",  # Vector similarity search extension
            "chromaprint",  # Audio fingerprinting
            "libvmaf",  # Video quality metrics
            "x264",  # H.264 encoder
            "vvdec",  # VVC decoder
            "vvenc",  # VVC encoder
        ]

        # Handle ffmpeg specially to avoid tap conflicts
        if not command_exists("ffmpeg"):
            print("Installing ffmpeg (standard version)...")
            print_c(
                DIM,
                "   💡 For full-featured ffmpeg, see script header for homebrew-ffmpeg tap instructions.",
            )
            run_cmd("brew install ffmpeg")
        else:
            ffmpeg_info = run_cmd("which ffmpeg", check=False, capture_output=True)
            print_c(
                GREEN,
                f"{pick_symbol('✅', ('[OK]'))} ffmpeg already installed at: {ffmpeg_info.stdout.strip() if ffmpeg_info.stdout else 'unknown'}",
            )
            print_c(DIM, "   Skipping to preserve existing installation.")

        for dep in deps:
            binary = dep
            if dep == "postgresql@14":
                binary = "psql"
            elif dep == "jpeg-xl":
                binary = "cjxl"
            elif dep == "libheif":
                binary = "heif-convert"
            elif dep == "libavif":
                binary = "avifdec"
            elif dep == "libvmaf":
                # libvmaf is a library, check via pkg-config
                if run_cmd("pkg-config --exists libvmaf", check=False).returncode == 0:
                    print_c(
                        GREEN, f"{pick_symbol('✅', ('[OK]'))} {dep} already installed."
                    )
                    continue
                binary = None

            if binary and command_exists(binary):
                print_c(
                    GREEN, f"{pick_symbol('✅', ('[OK]'))} {dep} already installed."
                )
            else:
                print(f"Installing {dep}...")
                run_cmd(f"brew install {dep}")

        # --- macOS Linker Workaround for libstdc++ ---
        print_c(
            BLUE,
            f"{pick_symbol('🔧', ('[TOOL]'))} Applying macOS linker workaround for libstdc++...",
        )
        tmp_lib_dir = "crates/.modern_format_boost/.tmp_lib"
        if not os.path.exists(tmp_lib_dir):
            os.makedirs(tmp_lib_dir)

        # 1. Create libstdc++.tbd pointing to system libc++.tbd in the SDK
        sdk_path = run_cmd("xcrun --show-sdk-path", capture_output=True).stdout.strip()
        libcxx_tbd = os.path.join(sdk_path, "usr/lib/libc++.tbd")
        target_tbd = os.path.join(tmp_lib_dir, "libstdc++.tbd")
        if os.path.exists(libcxx_tbd):
            run_cmd(f'ln -sf "{libcxx_tbd}" "{target_tbd}"')
            print_c(
                GREEN,
                f"   {pick_symbol('✅', ('[OK]'))} Linked libstdc++.tbd -> {libcxx_tbd}",
            )
        else:
            print_c(
                YELLOW, "   ⚠️  System libc++.tbd not found in SDK. Doctests might fail."
            )

        # 2. Create libstdc++.dylib pointing to system libc++.dylib
        target_dylib = os.path.join(tmp_lib_dir, "libstdc++.dylib")
        run_cmd(f'ln -sf "/usr/lib/libc++.dylib" "{target_dylib}"')
        print_c(
            GREEN,
            f"   {pick_symbol('✅', ('[OK]'))} Linked libstdc++.dylib -> /usr/lib/libc++.dylib",
        )

    elif OS_TYPE == "linux":
        print_c(YELLOW, f"{pick_symbol('🐧', ('[LINUX]'))} Detected Linux")
        if command_exists("apt-get"):
            print("Installing system dependencies via apt...")
            run_cmd("sudo apt-get update")
            run_cmd(
                "sudo apt-get install -y ffmpeg libimage-exiftool-perl imagemagick "
                "webp libheif-dev libavif-bin jpeginfo pngcheck exiv2 "
                "coreutils nodejs npm shellcheck shfmt curl git "
                "build-essential postgresql postgresql-contrib libchromaprint-dev "
                "libvmaf-dev pkg-config"
            )

            # Check for libjxl (JPEG XL)
            if not command_exists("cjxl"):
                print_c(
                    YELLOW,
                    f"{pick_symbol('⚠️', '[WARN]')}  JPEG XL tools not found in apt.",
                )
                print("   You may need to build from source or use a PPA:")
                print("   https://github.com/libjxl/libjxl")
        else:
            print_c(
                RED,
                "❌ Unsupported Linux distribution (apt not found). Please install dependencies manually.",
            )
            sys.exit(1)
    else:
        print_c(RED, f"{pick_symbol('❌', ('[ERROR]'))} Unsupported OS: {OS_TYPE}")
        sys.exit(1)

    if not command_exists("rustup"):
        print_c(
            YELLOW,
            f"{pick_symbol('🦀', ('[RUST]'))} Rust not found. Installing via rustup...",
        )
        run_cmd(
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
        )
        os.environ["PATH"] = (
            f"{os.environ.get('HOME')}/.cargo/bin:{os.environ.get('PATH')}"
        )
    else:
        print_c(GREEN, f"{pick_symbol('✅', ('[OK]'))} Rust found.")

    print("Updating Rust and adding components...")
    run_cmd("rustup update")
    run_cmd("rustup component add clippy rustfmt")

    print_c(BLUE, f"{pick_symbol('📦', ('[PKG]'))} Installing Cargo utilities...")
    cargo_tools = {
        "cargo-nextest": "cargo-nextest",  # Next-generation test runner
        "taplo-cli": "taplo",  # TOML formatter
        "cargo-bloat": "cargo-bloat",  # Binary size profiler
        "cargo-hack": "cargo-hack",  # Feature combination testing
        "cargo-audit": "cargo-audit",  # Security vulnerability scanner
        "dovi_tool": "dovi_tool",  # Dolby Vision metadata tool
        "hdr10plus_tool": "hdr10plus_tool",  # HDR10+ metadata tool
        "kondo": "kondo",  # Project cleanup tool
    }

    for package, binary in cargo_tools.items():
        if not command_exists(binary):
            print(f"Installing {package}...")
            run_cmd(f"cargo install {package}", check=False)
        else:
            print_c(
                GREEN, f"{pick_symbol('✅', ('[OK]'))} {package} already installed."
            )

    print_c(BLUE, f"{pick_symbol('🐍', ('[PYTHON]'))} Installing Python utilities...")
    if command_exists("pip3"):
        python_packages = [
            "ruff",  # Python linter and formatter
            "rich",  # Terminal formatting
            "psycopg2-binary",  # PostgreSQL adapter
            "tabulate",  # Table formatting
            "numpy",  # Numerical computing
            "pandas",  # Data analysis
            "scikit-learn",  # Machine learning
            "Pillow",  # Image processing
        ]
        print(f"   Installing: {', '.join(python_packages)}")
        run_cmd(f"pip3 install --upgrade {' '.join(python_packages)}", check=False)
    else:
        print_c(
            RED, f"{pick_symbol('⚠️', '[WARN]')}  pip3 not found. Skipping Python tools."
        )

    print_c(BLUE, f"{pick_symbol('🟢', ('[NODE]'))} Installing Node.js utilities...")
    if command_exists("npm"):
        print("Installing prettier and markdownlint-cli2 globally...")
        if OS_TYPE == "linux":
            run_cmd("sudo npm install -g prettier markdownlint-cli2", check=False)
        else:
            run_cmd("npm install -g prettier markdownlint-cli2", check=False)
    else:
        print_c(
            RED, f"{pick_symbol('⚠️', '[WARN]')}  npm not found. Skipping Node tools."
        )

    print("--------------------------------------------------------")
    print_c(
        GREEN,
        f"{pick_symbol('🌟', ('[STAR]'))} All dependencies installed successfully!",
    )
    print(
        "You can now run 'cargo run --locked -p dev --bin check_all' to verify the workspace."
    )
    print_c(
        DIM,
        "\n💡 Tip: For advanced FFmpeg features, see the script header for tap instructions.",
    )


if __name__ == "__main__":
    main()

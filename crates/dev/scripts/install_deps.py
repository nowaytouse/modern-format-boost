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
    print_c(BLUE, "🚀 Modern Format Boost - Dependency Installer v0.11.2")
    print("--------------------------------------------------------")
    print_c(
        DIM,
        "💡 For advanced FFmpeg setup (FDK-AAC, AI filters, etc.), see script header.",
    )
    print("--------------------------------------------------------\n")

    OS_TYPE = platform.system().lower()

    if OS_TYPE == "darwin":
        print_c(YELLOW, "🍎 Detected macOS")
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
            "exiftool",  # Metadata preservation
            "imagemagick",  # Image format conversion (magick)
            "webp",  # WebP support (dwebp, cwebp)
            "libheif",  # HEIF/HEIC support
            "coreutils",  # GNU core utilities
            "node",  # Node.js for prettier/markdownlint
            "shellcheck",  # Shell script linting
            "shfmt",  # Shell script formatting
            "postgresql@14",  # Database for ML training
            "pgvector",  # Vector similarity search extension
            "chromaprint",  # Audio fingerprinting
            "libvmaf",  # Video quality metrics
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
                f"✅ ffmpeg already installed at: {ffmpeg_info.stdout.strip() if ffmpeg_info.stdout else 'unknown'}",
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
            elif dep == "libvmaf":
                # libvmaf is a library, check via pkg-config
                if run_cmd("pkg-config --exists libvmaf", check=False).returncode == 0:
                    print_c(GREEN, f"✅ {dep} already installed.")
                    continue
                binary = None

            if binary and command_exists(binary):
                print_c(GREEN, f"✅ {dep} already installed.")
            else:
                print(f"Installing {dep}...")
                run_cmd(f"brew install {dep}")

    elif OS_TYPE == "linux":
        print_c(YELLOW, "🐧 Detected Linux")
        if command_exists("apt-get"):
            print("Installing system dependencies via apt...")
            run_cmd("sudo apt-get update")
            run_cmd(
                "sudo apt-get install -y ffmpeg libimage-exiftool-perl imagemagick "
                "webp libheif-dev coreutils nodejs npm shellcheck shfmt curl git "
                "build-essential postgresql postgresql-contrib libchromaprint-dev "
                "libvmaf-dev pkg-config"
            )

            # Check for libjxl (JPEG XL)
            if not command_exists("cjxl"):
                print_c(YELLOW, "⚠️  JPEG XL tools not found in apt.")
                print("   You may need to build from source or use a PPA:")
                print("   https://github.com/libjxl/libjxl")
        else:
            print_c(
                RED,
                "❌ Unsupported Linux distribution (apt not found). Please install dependencies manually.",
            )
            sys.exit(1)
    else:
        print_c(RED, f"❌ Unsupported OS: {OS_TYPE}")
        sys.exit(1)

    if not command_exists("rustup"):
        print_c(YELLOW, "🦀 Rust not found. Installing via rustup...")
        run_cmd(
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
        )
        os.environ["PATH"] = (
            f"{os.environ.get('HOME')}/.cargo/bin:{os.environ.get('PATH')}"
        )
    else:
        print_c(GREEN, "✅ Rust found.")

    print("Updating Rust and adding components...")
    run_cmd("rustup update")
    run_cmd("rustup component add clippy rustfmt")

    print_c(BLUE, "📦 Installing Cargo utilities...")
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
            print_c(GREEN, f"✅ {package} already installed.")

    print_c(BLUE, "🐍 Installing Python utilities...")
    if command_exists("pip3"):
        python_packages = [
            "ruff",  # Python linter and formatter
            "rich",  # Terminal formatting
            "psycopg2-binary",  # PostgreSQL adapter
            "tabulate",  # Table formatting
            "numpy",  # Numerical computing
            "pandas",  # Data analysis
            "scikit-learn",  # Machine learning
            "matplotlib",  # Plotting (for analysis.py)
            "imageio",  # Image/video I/O (for analysis.py)
            "Pillow",  # Image processing
        ]
        print(f"   Installing: {', '.join(python_packages)}")
        run_cmd(f"pip3 install --upgrade {' '.join(python_packages)}", check=False)
    else:
        print_c(RED, "⚠️  pip3 not found. Skipping Python tools.")

    print_c(BLUE, "🟢 Installing Node.js utilities...")
    if command_exists("npm"):
        print("Installing prettier and markdownlint-cli2 globally...")
        if OS_TYPE == "linux":
            run_cmd("sudo npm install -g prettier markdownlint-cli2", check=False)
        else:
            run_cmd("npm install -g prettier markdownlint-cli2", check=False)
    else:
        print_c(RED, "⚠️  npm not found. Skipping Node tools.")

    print("--------------------------------------------------------")
    print_c(GREEN, "🌟 All dependencies installed successfully!")
    print(
        "You can now run 'python3 crates/dev/scripts/check_all.py' to verify the workspace."
    )
    print_c(
        DIM,
        "\n💡 Tip: For advanced FFmpeg features, see the script header for tap instructions.",
    )


if __name__ == "__main__":
    main()

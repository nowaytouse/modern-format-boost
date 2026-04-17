#!/usr/bin/env python3
import sys
import subprocess
import shutil
import platform
import os

GREEN = "\033[0;32m"
BLUE = "\033[0;34m"
YELLOW = "\033[1;33m"
RED = "\033[0;31m"
NC = "\033[0m"


def print_c(color, text, end="\n"):
    print(f"{color}{text}{NC}", end=end)


def command_exists(cmd):
    return shutil.which(cmd) is not None


def run_cmd(cmd, check=True):
    return subprocess.run(cmd, shell=True, check=check)


def main():
    print_c(BLUE, "🚀 Modern Format Boost - Dependency Installer v0.11.2")
    print("--------------------------------------------------------")

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
            "jpeg-xl",
            "exiftool",
            "imagemagick",
            "webp",
            "libheif",
            "coreutils",
            "node",
            "shellcheck",
            "shfmt",
            "postgresql@14",
            "pgvector",
        ]

        if not command_exists("ffmpeg"):
            print("Installing ffmpeg...")
            run_cmd("brew install ffmpeg")
        else:
            print_c(
                GREEN, "✅ ffmpeg already installed (skipping to avoid tap conflicts)."
            )

        for dep in deps:
            binary = dep
            if dep == "postgresql@14":
                binary = "psql"
            if dep == "jpeg-xl":
                binary = "cjxl"

            if not command_exists(binary):
                print(f"Installing {dep}...")
                run_cmd(f"brew install {dep}")
            else:
                print_c(GREEN, f"✅ {dep} already installed.")

    elif OS_TYPE == "linux":
        print_c(YELLOW, "🐧 Detected Linux")
        if command_exists("apt-get"):
            print("Installing system dependencies via apt...")
            run_cmd("sudo apt-get update")
            run_cmd(
                "sudo apt-get install -y ffmpeg libimage-exiftool-perl imagemagick webp libheif-dev coreutils nodejs npm shellcheck shfmt curl git build-essential postgresql postgresql-contrib"
            )
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
        "cargo-nextest": "cargo-nextest",
        "taplo-cli": "taplo",
        "cargo-bloat": "cargo-bloat",
        "cargo-hack": "cargo-hack",
        "cargo-audit": "cargo-audit",
        "dovi_tool": "dovi_tool",
        "hdr10plus_tool": "hdr10plus_tool",
        "kondo": "kondo",
    }

    for package, binary in cargo_tools.items():
        if not command_exists(binary):
            print(f"Installing {package}...")
            run_cmd(f"cargo install {package}", check=False)
        else:
            print_c(GREEN, f"✅ {package} already installed.")

    print_c(BLUE, "🐍 Installing Python utilities...")
    if command_exists("pip3"):
        run_cmd("pip3 install --upgrade ruff rich", check=False)
    else:
        print_c(RED, "⚠️  pip3 not found. Skipping Python tools (ruff, rich).")

    print_c(BLUE, "🟢 Installing Node.js utilities...")
    if command_exists("npm"):
        print("Installing prettier and markdownlint-cli2 globally...")
        if OS_TYPE == "linux":
            run_cmd("sudo npm install -g prettier markdownlint-cli2", check=False)
        else:
            run_cmd("npm install -g prettier markdownlint-cli2", check=False)
    else:
        print_c(
            RED, "⚠️  npm not found. Skipping Node tools (prettier, markdownlint-cli2)."
        )

    print("--------------------------------------------------------")
    print_c(GREEN, "🌟 All dependencies installed successfully!")
    print("You can now run 'python3 scripts/check_all.py' to verify the workspace.")


if __name__ == "__main__":
    main()

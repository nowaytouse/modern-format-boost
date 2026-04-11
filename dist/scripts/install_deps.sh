#!/bin/bash
set -e

# --- Color Definitions ---
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

echo -e "${BLUE}🚀 Modern Format Boost - Dependency Installer v0.11.1${NC}"
echo "--------------------------------------------------------"

# --- OS Detection ---
OS_TYPE=$(uname -s | tr '[:upper:]' '[:lower:]')

# --- Helper Functions ---
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# --- System Package Installation ---
if [[ "$OS_TYPE" == "darwin" ]]; then
    echo -e "${YELLOW}🍎 Detected macOS${NC}"
    if ! command_exists brew; then
        echo "Installing Homebrew..."
        /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    fi
    echo "Updating Homebrew..."
    brew update
    echo "Installing system dependencies via Homebrew..."
    echo "Checking and installing system dependencies via Homebrew..."
    # Core dependencies
    deps=("jpeg-xl" "exiftool" "imagemagick" "webp" "libheif" "coreutils" "node" "shellcheck" "shfmt" "postgresql@14" "pgvector")

    # Special check for ffmpeg to avoid tap conflicts
    if ! command_exists ffmpeg; then
        echo "Installing ffmpeg..."
        brew install ffmpeg
    else
        echo -e "${GREEN}✅ ffmpeg already installed (skipping to avoid tap conflicts).${NC}"
    fi

    for dep in "${deps[@]}"; do
        # Map formula names to binary names if different
        binary=$dep
        [[ "$dep" == "postgresql@14" ]] && binary="psql"
        [[ "$dep" == "jpeg-xl" ]] && binary="cjxl"

        if ! command_exists "$binary"; then
            echo "Installing $dep..."
            brew install "$dep"
        else
            echo -e "${GREEN}✅ $dep already installed.${NC}"
        fi
    done
elif [[ "$OS_TYPE" == "linux" ]]; then
    echo -e "${YELLOW}🐧 Detected Linux${NC}"
    if command_exists apt-get; then
        echo "Installing system dependencies via apt..."
        sudo apt-get update
        sudo apt-get install -y ffmpeg libimage-exiftool-perl imagemagick webp libheif-dev coreutils nodejs npm shellcheck shfmt curl git build-essential postgresql postgresql-contrib
    else
        echo -e "${RED}❌ Unsupported Linux distribution (apt not found). Please install dependencies manually.${NC}"
        exit 1
    fi
else
    echo -e "${RED}❌ Unsupported OS: $OS_TYPE${NC}"
    exit 1
fi

# --- Rust Installation ---
if ! command_exists rustup; then
    echo -e "${YELLOW}🦀 Rust not found. Installing via rustup...${NC}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
else
    echo -e "${GREEN}✅ Rust found.${NC}"
fi

echo "Updating Rust and adding components..."
rustup update
rustup component add clippy rustfmt

# --- Cargo Tools ---
echo -e "${BLUE}📦 Installing Cargo utilities...${NC}"
# Binary name mapping for command_exists check
declare -A cargo_tools
cargo_tools["cargo-nextest"]="cargo-nextest"
cargo_tools["taplo-cli"]="taplo"
cargo_tools["cargo-bloat"]="cargo-bloat"
cargo_tools["cargo-hack"]="cargo-hack"
cargo_tools["cargo-audit"]="cargo-audit"
cargo_tools["dovi_tool"]="dovi_tool"
cargo_tools["hdr10plus_tool"]="hdr10plus_tool"
cargo_tools["kondo"]="kondo"

for package in "${!cargo_tools[@]}"; do
    binary=${cargo_tools[$package]}
    if ! command_exists "$binary"; then
        echo "Installing $package..."
        cargo install "$package"
    else
        echo -e "${GREEN}✅ $package already installed.${NC}"
    fi
done

# --- Python Tools ---
echo -e "${BLUE}🐍 Installing Python utilities...${NC}"
if command_exists pip3; then
    pip3 install --upgrade ruff rich
else
    echo -e "${RED}⚠️  pip3 not found. Skipping Python tools (ruff, rich).${NC}"
fi

# --- Node Tools ---
echo -e "${BLUE}🟢 Installing Node.js utilities...${NC}"
if command_exists npm; then
    echo "Installing prettier and markdownlint-cli2 globally..."
    if [[ "$OS_TYPE" == "linux" ]]; then
        sudo npm install -g prettier markdownlint-cli2
    else
        npm install -g prettier markdownlint-cli2
    fi
else
    echo -e "${RED}⚠️  npm not found. Skipping Node tools (prettier, markdownlint-cli2).${NC}"
fi

echo "--------------------------------------------------------"
echo -e "${GREEN}🌟 All dependencies installed successfully!${NC}"
echo "You can now run 'python3 scripts/check_all.py' to verify the workspace."

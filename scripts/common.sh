# common.sh - 路径与颜色等公共定义
# 使用前请在脚本中设置 SCRIPT_DIR，再 source "$SCRIPT_DIR/common.sh"

# 若未设置 SCRIPT_DIR，尝试从调用者推断（bash: BASH_SOURCE[0]，zsh: %x）
if [[ -z "${SCRIPT_DIR:-}" ]]; then
    if [[ -n "${BASH_SOURCE[0]:-}" ]]; then
        SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    else
        # zsh: ${(%):-%x} 为当前脚本路径
        SCRIPT_DIR="$(cd "$(dirname "${(%):-%x}")" && pwd)"
    fi
fi

PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 颜色（兼容 bash 与 zsh；256 色码在多数终端可用）
RESET='\033[0m'
NC='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
RED='\033[38;5;196m'
GREEN='\033[38;5;46m'
YELLOW='\033[38;5;226m'
BLUE='\033[38;5;39m'
CYAN='\033[38;5;51m'
MAGENTA='\033[38;5;213m'
WHITE='\033[38;5;255m'
GRAY='\033[38;5;240m'
BG_HEADER='\033[48;5;236m'

# Common Utilities Module (通用工具模块)

🔥 **v7.8**: Extracted common patterns from across the project

## Purpose (目的)

Consolidates duplicate code patterns into reusable utility functions:
- File operations (文件操作)
- String processing (字符串处理)  
- Command execution (命令执行)

## Functions (函数列表)

### File Operations
- `get_extension_lowercase()` - Safe extension extraction
- `has_extension()` - Check file extension
- `is_hidden_file()` - Detect hidden files
- `ensure_dir_exists()` - Create directories safely
- `ensure_parent_dir_exists()` - Create parent directories
- `compute_relative_path()` - Calculate relative paths
- `copy_file_with_context()` - Copy with error context

### String Processing
- `normalize_path_string()` - Normalize path separators
- `truncate_string()` - Truncate with ellipsis
- `extract_digits()` - Extract numeric characters
- `parse_float_or_default()` - Safe float parsing

### Command Execution
- `execute_command_with_logging()` - Execute with full logging
- `is_command_available()` - Check command availability
- `get_command_version()` - Get command version
- `format_command_string()` - Format for logging

## Usage (使用方法)

```rust
use shared_utils::common_utils::*;

// File operations
let ext = get_extension_lowercase(Path::new("test.JPG")); // "jpg"
ensure_dir_exists(Path::new("/tmp/test"))?;

// String processing
let normalized = normalize_path_string("C:\\Users\\test"); // "C:/Users/test"

// Command execution
if is_command_available("ffmpeg") {
    let mut cmd = Command::new("ffmpeg");
    execute_command_with_logging(&mut cmd)?;
}
```

## Testing (测试)

```bash
cargo test --package shared_utils common_utils
```

All 14 tests pass ✅

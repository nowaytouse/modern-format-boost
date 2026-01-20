# External Command Logging Usage Guide

## Overview

The logging module provides utilities for executing and logging external commands (ffmpeg, x265, etc.) with automatic capture of stdout/stderr and execution metrics.

## API Functions

### 1. `log_external_tool()`

Records external tool execution details to the log.

```rust
pub fn log_external_tool(
    tool_name: &str,
    args: &[&str],
    output: &str,
    exit_code: Option<i32>,
    duration: std::time::Duration,
)
```

**Example:**
```rust
use shared_utils::logging::log_external_tool;
use std::time::Duration;

log_external_tool(
    "ffmpeg",
    &["-i", "input.mp4", "output.mp4"],
    "ffmpeg version 6.0...",
    Some(0),
    Duration::from_secs(10),
);
```

### 2. `execute_external_command()`

Executes an external command and automatically logs all details.

```rust
pub fn execute_external_command(
    tool_name: &str,
    args: &[&str],
) -> Result<ExternalCommandResult>
```

**Example:**
```rust
use shared_utils::logging::execute_external_command;

let result = execute_external_command("ffmpeg", &["-version"])?;
println!("Exit code: {:?}", result.exit_code);
println!("Output: {}", result.stdout);
println!("Duration: {:?}", result.duration);
```

### 3. `execute_external_command_checked()`

Executes a command and returns an error if it fails.

```rust
pub fn execute_external_command_checked(
    tool_name: &str,
    args: &[&str],
) -> Result<ExternalCommandResult>
```

**Example:**
```rust
use shared_utils::logging::execute_external_command_checked;

// Automatically fails if exit code != 0
let result = execute_external_command_checked("ffmpeg", &["-i", "input.mp4", "output.mp4"])?;
println!("Success! Took {:?}", result.duration);
```

## ExternalCommandResult Structure

```rust
pub struct ExternalCommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration: std::time::Duration,
}
```

## Log Output Format

Successful execution:
```
INFO tool=ffmpeg command="ffmpeg -version" duration_secs=0.123 exit_code=0 "External tool completed successfully"
```

Failed execution:
```
ERROR tool=ffmpeg command="ffmpeg -i bad.mp4" duration_secs=0.456 exit_code=1 output="Error: No such file" "External tool failed"
```

## Requirements Validated

- **Requirement 2.10**: Records all external tool invocations
- **Requirement 16.2**: Logs process startup, execution, and exit status
- **Requirement 16.3**: Captures complete command line, stdout, and stderr

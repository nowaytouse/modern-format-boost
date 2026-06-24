//! Raw terminal key input and drag/drop path helpers.
//! Mirrors `read_key()`, `unescape_path()`, and `resize_terminal()` in
//! `drag_and_drop_processor.py`.

use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

/// Arrow / navigation keys from raw stdin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Left,
    Right,
    Tab,
    Enter,
    Quit,
    Char(char),
    Unknown,
}

impl NavKey {
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match bytes {
            [b'q'] => Self::Quit,
            [b'\t'] => Self::Tab,
            [b'\r'] | [b'\n'] => Self::Enter,
            [b'0'..=b'9'] => Self::Char(bytes[0] as char),
            [0x1b, b'[', b'A'] | [0x1b, b'D'] => Self::Up,
            [0x1b, b'[', b'B'] => Self::Down,
            [0x1b, b'[', b'C'] => Self::Right,
            [0x1b, b'[', b'D'] => Self::Left,
            [c] if c.is_ascii_graphic() || *c == b' ' => Self::Char(*c as char),
            _ => Self::Unknown,
        }
    }
}

/// Read one navigation key from TTY (blocking). Falls back to line-based read
/// when not a TTY.
pub fn read_nav_key() -> Result<NavKey> {
    if !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(NavKey::Enter);
        }
        if trimmed == "q" {
            return Ok(NavKey::Quit);
        }
        if trimmed.len() == 1 {
            return Ok(NavKey::Char(trimmed.chars().next().unwrap_or('\n')));
        }
        return Ok(NavKey::Unknown);
    }

    #[cfg(unix)]
    {
        read_nav_key_unix()
    }
    #[cfg(not(unix))]
    {
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Ok(NavKey::from_bytes(line.trim().as_bytes()))
    }
}

#[cfg(unix)]
fn read_nav_key_unix() -> Result<NavKey> {
    use std::os::unix::io::AsRawFd;

    let fd = io::stdin().as_raw_fd();
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
        anyhow::bail!("tcgetattr failed");
    }
    let saved = termios;
    let mut raw = termios;
    raw.c_lflag &= !(libc::ICANON | libc::ECHO);
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    if unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, &raw) } != 0 {
        anyhow::bail!("tcsetattr raw failed");
    }

    let result = (|| -> Result<NavKey> {
        let mut buf = [0u8; 8];
        let n = io::stdin().read(&mut buf).context("read nav key")?;
        if n == 0 {
            return Ok(NavKey::Unknown);
        }
        if buf[0] == 0x1b && n < 3 {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags >= 0 {
                unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
            }
            let extra = match io::stdin().read(&mut buf[n..]) {
                Ok(n) => n,
                Err(err) => {
                    eprintln!("[INPUT] extra nav key read failed: {err}");
                    0
                }
            };
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
            Ok(NavKey::from_bytes(&buf[..n + extra]))
        } else {
            Ok(NavKey::from_bytes(&buf[..n]))
        }
    })();

    unsafe {
        libc::tcsetattr(fd, libc::TCSADRAIN, &saved);
    }
    result
}

/// Handle shell-escaped paths common in terminal drag-and-drop.
pub fn unescape_path(path_str: &str) -> String {
    let mut path_str = path_str.trim();
    if path_str.is_empty() {
        return String::new();
    }
    path_str = path_str.trim_matches('"').trim_matches('\'');
    if path_str.contains('\\') && !Path::new(path_str).exists() {
        return path_str
            .replace("\\ ", " ")
            .replace("\\!", "!")
            .replace("\\&", "&")
            .replace("\\(", "(")
            .replace("\\)", ")")
            .replace("\\'", "'")
            .replace("\\\"", "\"");
    }
    path_str.to_string()
}

/// Validate drag/drop target path (mirrors Python `get_target_directory`
/// guards).
pub fn validate_drag_drop_path(raw: &str) -> Result<PathBuf> {
    if raw.contains('\n') || raw.contains('\r') {
        anyhow::bail!("path contains unsupported control characters");
    }
    let cleaned = unescape_path(raw);
    if cleaned.is_empty() {
        anyhow::bail!("empty path");
    }
    let path = PathBuf::from(&cleaned);
    if !path.exists() {
        anyhow::bail!("path not found: {}", path.display());
    }
    Ok(path)
}

/// Resize terminal to wide format for double-click launches (ANSI DECSTBM).
pub fn resize_terminal_for_gui(rows: u16, cols: u16) {
    if io::stdout().is_terminal() {
        let _ = write!(io::stdout(), "\x1B[8;{rows};{cols}t");
        let _ = io::stdout().flush();
    }
}

/// Flush stdin to prevent accidental menu triggers (mirrors Python
/// `drain_stdin`).
#[cfg(unix)]
pub fn drain_stdin() {
    use std::os::unix::io::AsRawFd;
    let fd = io::stdin().as_raw_fd();
    unsafe {
        libc::tcflush(fd, libc::TCIFLUSH);
    }
}

#[cfg(not(unix))]
pub fn drain_stdin() {}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn unescape_strips_surrounding_quotes() {
        assert_eq!(unescape_path("\"/Users/test/Album\""), "/Users/test/Album");
    }

    #[test]
    fn unescape_replaces_backslash_space() {
        assert_eq!(
            unescape_path("/Users/test/My\\ Album"),
            "/Users/test/My Album"
        );
    }
}

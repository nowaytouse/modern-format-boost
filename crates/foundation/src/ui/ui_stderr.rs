//! Central stderr presentation for the terminal UI layer (U7).
//!
//! All user-facing stderr lines should use [`line()`] / [`line_fmt`] with
//! [`crate::modern_ui::symbols::pick`] rather than embedding emoji literals at
//! call sites.

use crate::modern_ui::symbols;
use crate::progress_mode;
use std::fmt::Display;

/// Emit one stderr line with emoji or ASCII icon per
/// [`progress_mode::is_plain_mode`].
pub fn line(icon_emoji: &str, icon_plain: &str, msg: impl Display) {
    let icon = symbols::pick(icon_emoji, icon_plain);
    progress_mode::emit_stderr(&format!("{icon} {msg}"));
}

/// Section header (leading newline preserved when present in `title`).
pub fn section(icon_emoji: &str, icon_plain: &str, title: impl Display) {
    line(icon_emoji, icon_plain, title);
}

/// Horizontal rule for report-style blocks.
pub fn rule() {
    let text = if progress_mode::is_plain_mode() {
        "=".repeat(60)
    } else {
        "━".repeat(60)
    };
    progress_mode::emit_stderr(&text);
}

/// Same as [`line()`] with `format!`-style arguments.
pub fn line_fmt(icon_emoji: &str, icon_plain: &str, args: std::fmt::Arguments<'_>) {
    line(icon_emoji, icon_plain, format_args!("{args}"));
}

#[macro_export]
macro_rules! ui_stderr_line {
    ($emoji:expr, $plain:expr, $($arg:tt)+) => {
        $crate::ui_stderr::line_fmt($emoji, $plain, format_args!($($arg)+))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_uses_plain_symbol_when_plain_mode() {
        progress_mode::set_plain_mode(true);
        // line() must not panic; pick is exercised via symbols unit test
        line(symbols::CHECK, symbols::plain::CHECK, "ok");
        progress_mode::set_plain_mode(false);
    }
}

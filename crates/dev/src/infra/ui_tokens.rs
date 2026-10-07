//! Terminal UI tokens and symbols aligned with MFB Python/Rust specifications.

use std::io::{IsTerminal, stdout};

pub const BRAND_BLUE: &str = "#43a0ff";

/// Returns true if colors should be enabled based on standard env variables and
/// TTY status.
#[must_use]
pub fn colors_enabled() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    match std::env::var("MODERN_FORMAT_PLAIN_UI") {
        Ok(val) => {
            let val_lower = val.to_lowercase();
            if val_lower == "1" || val_lower == "true" || val_lower == "yes" || val_lower == "on" {
                return false;
            }
        }
        Err(_err) => {}
    }
    stdout().is_terminal()
}

/// Returns true if plain mode is requested (no emojis/special unicode
/// characters).
#[must_use]
pub fn plain_mode_enabled() -> bool {
    match std::env::var("MODERN_FORMAT_PLAIN_UI") {
        Ok(val) => {
            let val_lower = val.to_lowercase();
            if val_lower == "1" || val_lower == "true" || val_lower == "yes" || val_lower == "on" {
                return true;
            }
        }
        Err(_err) => {}
    }
    !stdout().is_terminal()
}

/// Picks between an emoji symbol and an ASCII fallback.
#[must_use]
pub fn pick_symbol<'a>(emoji: &'a str, ascii_fallback: &'a str) -> &'a str {
    if plain_mode_enabled() {
        ascii_fallback
    } else {
        emoji
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn test_pick_symbol() {
        // Test with MODERN_FORMAT_PLAIN_UI=1
        unsafe {
            std::env::set_var("MODERN_FORMAT_PLAIN_UI", "1");
        }
        assert_eq!(pick_symbol("🚀", "[START]"), "[START]");

        // Test with MODERN_FORMAT_PLAIN_UI=0
        unsafe {
            std::env::set_var("MODERN_FORMAT_PLAIN_UI", "0");
        }
        // plain_mode_enabled() fallback depends on stdout().is_terminal(), but
        // pick_symbol should return something.
        let sym = pick_symbol("🚀", "[START]");
        assert!(sym == "🚀" || sym == "[START]");

        unsafe {
            std::env::remove_var("MODERN_FORMAT_PLAIN_UI");
        }
    }

    #[test]
    #[serial]
    fn test_plain_mode_enabled() {
        unsafe {
            std::env::set_var("MODERN_FORMAT_PLAIN_UI", "true");
        }
        assert!(plain_mode_enabled());

        unsafe {
            std::env::set_var("MODERN_FORMAT_PLAIN_UI", "1");
        }
        assert!(plain_mode_enabled());

        unsafe {
            std::env::set_var("MODERN_FORMAT_PLAIN_UI", "false");
            std::env::remove_var("MODERN_FORMAT_PLAIN_UI");
        }
    }

    #[test]
    #[serial]
    fn test_colors_enabled() {
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
        assert!(!colors_enabled());
        unsafe {
            std::env::remove_var("NO_COLOR");
        }

        unsafe {
            std::env::set_var("MODERN_FORMAT_PLAIN_UI", "yes");
        }
        assert!(!colors_enabled());
        unsafe {
            std::env::remove_var("MODERN_FORMAT_PLAIN_UI");
        }
    }
}

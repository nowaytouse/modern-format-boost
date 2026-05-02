//! Modern UI/UX Module
//!
//! Provides modern terminal interactions and visual effects:
//! - Dynamic Spinner animations
//! - Gradient progress bars
//! - Real-time status updates
//! - Beautified result presentation

use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// ANSI color and styling constants for terminal output.
pub mod colors {
    /// Reset all terminal attributes.
    pub const RESET: &str = "\x1b[0m";
    /// Enable bold text.
    pub const BOLD: &str = "\x1b[1m";
    /// Enable dimmed text.
    pub const DIM: &str = "\x1b[2m";
    /// Enable italic text.
    pub const ITALIC: &str = "\x1b[3m";
    /// Enable underlined text.
    pub const UNDERLINE: &str = "\x1b[4m";

    /// Standard red foreground color.
    pub const RED: &str = "\x1b[31m";
    /// Standard green foreground color.
    pub const GREEN: &str = "\x1b[32m";
    /// Standard yellow foreground color.
    pub const YELLOW: &str = "\x1b[33m";
    /// Standard blue foreground color.
    pub const BLUE: &str = "\x1b[34m";
    /// Standard magenta foreground color.
    pub const MAGENTA: &str = "\x1b[35m";
    /// Standard cyan foreground color.
    pub const CYAN: &str = "\x1b[36m";
    /// Standard white foreground color.
    pub const WHITE: &str = "\x1b[37m";

    /// Bright red foreground color.
    pub const BRIGHT_RED: &str = "\x1b[91m";
    /// Bright green foreground color.
    pub const BRIGHT_GREEN: &str = "\x1b[92m";
    /// Bright yellow foreground color.
    pub const BRIGHT_YELLOW: &str = "\x1b[93m";
    /// Bright blue foreground color.
    pub const BRIGHT_BLUE: &str = "\x1b[94m";
    /// Bright magenta foreground color.
    pub const BRIGHT_MAGENTA: &str = "\x1b[95m";
    /// Bright cyan foreground color.
    pub const BRIGHT_CYAN: &str = "\x1b[96m";
    /// Bright white foreground color.
    pub const BRIGHT_WHITE: &str = "\x1b[97m";

    /// Black background color.
    pub const BG_BLACK: &str = "\x1b[40m";
    /// Red background color.
    pub const BG_RED: &str = "\x1b[41m";
    /// Green background color.
    pub const BG_GREEN: &str = "\x1b[42m";
    /// Yellow background color.
    pub const BG_YELLOW: &str = "\x1b[43m";
    /// Blue background color.
    pub const BG_BLUE: &str = "\x1b[44m";
    /// Magenta background color.
    pub const BG_MAGENTA: &str = "\x1b[45m";
    /// Cyan background color.
    pub const BG_CYAN: &str = "\x1b[46m";
    /// White background color.
    pub const BG_WHITE: &str = "\x1b[47m";

    /// Modern Format Boost blue — true-color RGB (67, 160, 255).
    pub const MFB_BLUE: &str = "\x1b[38;2;67;160;255m";
    /// Modern Format Boost purple — true-color RGB (187, 134, 252).
    pub const MFB_PURPLE: &str = "\x1b[38;2;187;134;252m";
    /// Modern Format Boost pink — true-color RGB (255, 121, 198).
    pub const MFB_PINK: &str = "\x1b[38;2;255;121;198m";
    /// Modern Format Boost green — true-color RGB (80, 250, 123).
    pub const MFB_GREEN: &str = "\x1b[38;2;80;250;123m";
    /// Modern Format Boost yellow — true-color RGB (241, 250, 140).
    pub const MFB_YELLOW: &str = "\x1b[38;2;241;250;140m";
    /// Modern Format Boost cyan — true-color RGB (139, 233, 253).
    pub const MFB_CYAN: &str = "\x1b[38;2;139;233;253m";
    /// Modern Format Boost orange — true-color RGB (255, 184, 108).
    pub const MFB_ORANGE: &str = "\x1b[38;2;255;184;108m";
    /// Modern Format Boost red — true-color RGB (255, 85, 85).
    pub const MFB_RED: &str = "\x1b[38;2;255;85;85m";

    /// Primary accent color for UI highlights.
    pub const ACCENT: &str = "\x1b[38;2;0;198;255m";
    /// Gradient color used for success messages.
    pub const SUCCESS_GRADIENT: &str = "\x1b[38;2;0;255;135m";
    /// Gradient color used for warning messages.
    pub const WARNING_GRADIENT: &str = "\x1b[38;2;255;220;0m";
    /// Gradient color used for error messages.
    pub const ERROR_GRADIENT: &str = "\x1b[38;2;255;75;43m";
    /// Muted/dimmed gray for secondary text.
    pub const MUTE: &str = "\x1b[38;2;100;100;100m";
}

/// Helper functions for applying color gradient effects to text.
pub mod gradients {
    use super::colors::{MFB_BLUE, MFB_PURPLE, RESET};

    /// Wraps `text` in the MFB blue color code.
    #[must_use]
    pub fn blue_to_cyan(text: &str) -> String {
        format!("{MFB_BLUE}{text}{RESET}") // Simplified for now, real gradient logic would iterate chars
    }

    /// Wraps `text` in the MFB purple color code.
    #[must_use]
    pub fn purple_to_pink(text: &str) -> String {
        format!("{MFB_PURPLE}{text}{RESET}")
    }
}

/// Unicode symbols and icon-like characters used throughout the CLI output.
pub mod symbols {
    /// Check mark symbol.
    pub const CHECK: &str = "v";
    /// Cross / failure symbol.
    pub const CROSS: &str = "x";
    /// Right-pointing arrow.
    pub const ARROW_RIGHT: &str = "->";
    /// Down-pointing arrow.
    pub const ARROW_DOWN: &str = "v";
    /// Bullet point.
    pub const BULLET: &str = "*";
    /// Star symbol.
    pub const STAR: &str = "*";
    /// Sparkle symbol.
    pub const SPARKLE: &str = "";
    /// Fire symbol.
    pub const FIRE: &str = "";
    /// Rocket symbol.
    pub const ROCKET: &str = "";
    /// Search / magnifying glass symbol.
    pub const SEARCH: &str = "";
    /// Chart / graph symbol.
    pub const CHART: &str = "";
    /// Folder symbol.
    pub const FOLDER: &str = "";
    /// Video symbol.
    pub const VIDEO: &str = "";
    /// Image symbol.
    pub const IMAGE: &str = "";
    /// Compress / squeeze symbol.
    pub const COMPRESS: &str = "";
    /// Quality indicator symbol.
    pub const QUALITY: &str = "";
    /// GPU indicator symbol.
    pub const GPU: &str = "";
    /// CPU indicator symbol.
    pub const CPU: &str = "";
    /// Clock / time indicator symbol.
    pub const CLOCK: &str = "";
    /// Save / disk symbol.
    pub const SAVE: &str = "";
    /// Warning indicator symbol.
    pub const WARNING: &str = "!";
    /// Error indicator symbol.
    pub const ERROR: &str = "X";
    /// Success indicator symbol.
    pub const SUCCESS: &str = "OK";
    /// Information indicator symbol.
    pub const INFO: &str = "i";
    /// Diamond / bullet point symbol.
    pub const DIAMOND: &str = ">";
    /// Medal / achievement symbol.
    pub const MEDAL: &str = "";
    /// Shield / security symbol.
    pub const SHIELD: &str = "";
    /// Link / chain symbol.
    pub const LINK: &str = "";
    /// Bug / error symbol.
    pub const BUG: &str = "!";
    /// Stop / halt symbol.
    pub const STOP: &str = "!";
}

/// Constants that define the appearance of progress bars and spinners.
pub mod progress_style {
    /// Characters used to represent filled, boundary, and empty portions of a progress bar.
    pub const PROGRESS_CHARS: &str = "=#-";

    /// Default width of the progress bar in characters.
    pub const BAR_WIDTH: usize = 35;

    /// Character displayed at the left edge of the progress bar.
    pub const BAR_LEFT: &str = "[";
    /// Character displayed at the right edge of the progress bar.
    pub const BAR_RIGHT: &str = "]";

    /// Characters used for the animated spinner frames (dash, forward slash, pipe, backslash).
    pub const SPINNER_CHARS: &str = "-/|\\";

    /// Template string for batch-processing progress (used with `indicatif`).
    pub const BATCH_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} [{bar:35.green/black}] {percent:>3}% * {pos}/{len} * {elapsed_precise} (ETA: {eta}) * {msg}";

    /// Template string for explore-mode progress (used with `indicatif`).
    pub const EXPLORE_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} [{bar:35.green/black}] {percent:>3}% * {elapsed} * {msg}";

    /// Template string for compact progress display (used with `indicatif`).
    pub const COMPACT_TEMPLATE: &str =
        "{prefix:.cyan} [{bar:30.green/black}] {percent:>3}% ({pos}/{len}) {msg:.dim}";

    /// Template string for spinner-only display (used with `indicatif`).
    pub const SPINNER_TEMPLATE: &str = "{spinner:.green} {prefix:.cyan.bold} * {elapsed} * {msg}";
}

const SPINNER_FRAMES: &[&str] = &["-", "/", "|", "\\"];
const SPINNER_DOTS: &[&str] = &["*", ".", "o", "O"];

static SPINNER_FRAME: AtomicU64 = AtomicU64::new(0);

/// Returns the next spinner animation frame (rotating through dash, slash, pipe, backslash).
pub fn spinner_frame() -> &'static str {
    let frame = usize::try_from(SPINNER_FRAME.fetch_add(1, Ordering::Relaxed)).unwrap_or(0);
    SPINNER_FRAMES
        .get(frame % SPINNER_FRAMES.len())
        .copied()
        .unwrap_or("-")
}

/// Returns the next spinner dots animation frame (rotating through asterisk, dot, small o, capital O).
pub fn spinner_dots() -> &'static str {
    let frame = usize::try_from(SPINNER_FRAME.fetch_add(1, Ordering::Relaxed)).unwrap_or(0);
    SPINNER_DOTS
        .get(frame % SPINNER_DOTS.len())
        .copied()
        .unwrap_or("*")
}

/// Visual style variants for progress bar rendering.
#[derive(Clone, Copy)]
pub enum ProgressStyle {
    /// Classic Unicode block-progress using filled/empty block characters.
    Classic,
    /// Minimalist ASCII-style using equals signs and dashes.
    Modern,
    /// Shaded Unicode blocks with a gradient-like edge effect.
    Gradient,
    /// Fine-grained Unicode blocks with smooth fill transitions.
    Blocks,
}

/// Renders a text-based progress bar of the given `width`.
///
/// `progress` is clamped to the range `0.0..=1.0`, where `1.0` represents 100%.
/// The visual appearance is determined by the `style` variant.
#[must_use]
pub fn render_progress_bar(progress: f64, width: usize, style: ProgressStyle) -> String {
    let progress = progress.clamp(0.0, 1.0);
    let filled = crate::numeric_cast::f64_to_usize_sat(
        (progress * crate::numeric_cast::usize_to_f64(width)).round(),
    );
    let empty = width.saturating_sub(filled);

    match style {
        ProgressStyle::Classic => {
            format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
        }
        ProgressStyle::Modern => {
            format!("{}{}", "=".repeat(filled), "-".repeat(empty))
        }
        ProgressStyle::Gradient => {
            let mut bar = String::new();
            for i in 0..width {
                if i < filled {
                    bar.push('▓');
                } else if i == filled && progress > 0.0 {
                    bar.push('▒');
                } else {
                    bar.push('░');
                }
            }
            bar
        }
        ProgressStyle::Blocks => {
            let mut bar = String::new();
            for i in 0..width {
                let pos =
                    crate::numeric_cast::usize_to_f64(i) / crate::numeric_cast::usize_to_f64(width);
                if pos < progress - 0.1 {
                    bar.push('█');
                } else if pos < progress - 0.05 {
                    bar.push('▓');
                } else if pos < progress {
                    bar.push('▒');
                } else {
                    bar.push('░');
                }
            }
            bar
        }
    }
}

/// Renders a progress bar with color-coded segments based on completion percentage.
///
/// The bar color transitions from red (low) to yellow, cyan, and green (high).
#[must_use]
pub fn render_colored_progress(progress: f64, width: usize) -> String {
    use colors::{BRIGHT_CYAN, BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, RESET};

    let bar = render_progress_bar(progress, width, ProgressStyle::Modern);
    let pct = crate::numeric_cast::f64_to_u32_sat(progress * 100.0);

    let color = if pct >= 80 {
        BRIGHT_GREEN
    } else if pct >= 50 {
        BRIGHT_CYAN
    } else if pct >= 25 {
        BRIGHT_YELLOW
    } else {
        BRIGHT_RED
    };

    format!("{color}{bar}{RESET}")
}

/// Tracks the state of a CRF exploration progress, displaying real-time updates.
pub struct ExploreProgressState {
    /// Current stage label (e.g., "Exploring", "Finalizing").
    pub stage: String,
    /// Current CRF (Constant Rate Factor) value being tested.
    pub crf: f32,
    /// Percentage change in file size relative to the original.
    pub size_pct: f64,
    /// Structural Similarity Index Measure, if computed.
    pub ssim: Option<f64>,
    /// Number of iterations completed so far.
    pub iteration: u32,
    /// The best CRF found so far (one that reduced file size).
    pub best_crf: Option<f32>,
    /// The instant when this exploration started.
    pub start_time: Instant,
}

impl ExploreProgressState {
    /// Creates a new `ExploreProgressState` with the given `stage` label.
    #[must_use]
    pub fn new(stage: &str) -> Self {
        Self {
            stage: stage.to_string(),
            crf: 0.0,
            size_pct: 0.0,
            ssim: None,
            iteration: 0,
            best_crf: None,
            start_time: Instant::now(),
        }
    }

    /// Updates the state with new CRF, size percentage, and optional SSIM values,
    /// then prints the updated progress to stderr.
    pub fn update(&mut self, crf: f32, size_pct: f64, ssim: Option<f64>) {
        self.crf = crf;
        self.size_pct = size_pct;
        self.ssim = ssim;
        self.iteration += 1;

        if size_pct < 0.0 {
            self.best_crf = Some(crf);
        }

        self.display();
    }

    /// Displays the current progress inline on stderr, overwriting the previous line.
    pub fn display(&self) {
        use colors::{BRIGHT_GREEN, BRIGHT_YELLOW, CYAN, DIM, RESET};
        use symbols::{BULLET, SAVE, WARNING};

        // Pause output if the Ctrl+C confirmation prompt is currently waiting for input
        crate::ctrlc_guard::wait_if_prompt_active();

        let elapsed = self.start_time.elapsed().as_secs_f64();

        let (_size_icon, size_color) = if self.size_pct < 0.0 {
            (SAVE, BRIGHT_GREEN)
        } else {
            (WARNING, BRIGHT_YELLOW)
        };

        let ssim_str = self
            .ssim
            .map(|s| format!(" {DIM}SSIM {s:.4}{RESET}"))
            .unwrap_or_default();

        let best_str = self
            .best_crf
            .map(|b| format!(" {DIM}Best: {b:.1}{RESET}"))
            .unwrap_or_default();

        eprint!(
            "\r\x1b[K{} {}{}{} {} CRF {:.1} {} {}{:+.1}%{}{}{} {} {}{:.1}s{}",
            spinner_frame(),
            CYAN,
            self.stage,
            RESET,
            BULLET,
            self.crf,
            BULLET,
            size_color,
            self.size_pct,
            RESET,
            ssim_str,
            best_str,
            BULLET,
            DIM,
            elapsed,
            RESET
        );
        let _ = io::stderr().flush();
    }

    /// Prints a final summary of the exploration results, including quality rating.
    pub fn finish(&self, final_crf: f32, final_size_pct: f64, final_ssim: Option<f64>) {
        use colors::{BOLD, BRIGHT_GREEN, BRIGHT_YELLOW, RESET};
        use symbols::{BULLET, CHECK, SAVE, SUCCESS, WARNING};

        let elapsed = self.start_time.elapsed().as_secs_f64();

        eprint!("\r\x1b[K");

        let (ssim_str, ssim_rating) = match final_ssim {
            Some(s) if s >= 0.99 => (format!("SSIM {s:.4}"), format!("{SUCCESS} Excellent")),
            Some(s) if s >= 0.98 => (format!("SSIM {s:.4}"), format!("{SUCCESS} Very Good")),
            Some(s) if s >= 0.95 => (format!("SSIM {s:.4}"), format!("{CHECK}  Good")),
            Some(s) => (format!("SSIM {s:.4}"), format!("{WARNING}  Fair")),
            None => (String::new(), String::new()),
        };

        let size_str = if final_size_pct < 0.0 {
            format!("{BRIGHT_GREEN}{final_size_pct:+.1}%{RESET} {SAVE}")
        } else {
            format!("{BRIGHT_YELLOW}{final_size_pct:+.1}%{RESET}")
        };

        eprintln!(
            "{} {}Result:{} CRF {:.1} {} {} {} {} {} {} iter {} {:.1}s",
            SUCCESS,
            BOLD,
            RESET,
            final_crf,
            BULLET,
            size_str,
            BULLET,
            ssim_str,
            ssim_rating,
            BULLET,
            self.iteration,
            elapsed
        );
    }
}

/// Prints a boxed result with a decorative border, centered title, and content lines.
pub fn print_result_box(title: &str, lines: &[&str]) {
    use colors::{BOLD, BRIGHT_WHITE, MFB_BLUE, RESET};

    let max_width = lines
        .iter()
        .map(|l| strip_ansi(l).len())
        .max()
        .unwrap_or(40)
        .max(strip_ansi(title).len())
        .max(40);

    let box_width = max_width + 6;
    let theme_color = MFB_BLUE;

    eprintln!("{}╔{}╗{}", theme_color, "═".repeat(box_width), RESET);

    let title_stripped = strip_ansi(title);
    let title_padding_total = box_width - title_stripped.len() - 2;
    let title_padding_left = title_padding_total / 2;
    let title_padding_right = title_padding_total - title_padding_left;

    eprintln!(
        "{}║{} {}{}{}{} {}{}║{}",
        theme_color,
        " ".repeat(title_padding_left),
        BOLD,
        BRIGHT_WHITE,
        title,
        RESET,
        " ".repeat(title_padding_right),
        theme_color,
        RESET
    );

    eprintln!("{}╠{}╣{}", theme_color, "═".repeat(box_width), RESET);

    for line in lines {
        let line_stripped = strip_ansi(line);
        let padding = box_width - line_stripped.len() - 2;
        eprintln!(
            "{}║{} {}{} {}║{}",
            theme_color,
            RESET,
            line,
            " ".repeat(padding),
            theme_color,
            RESET
        );
    }

    eprintln!("{}╚{}╝{}", theme_color, "═".repeat(box_width), RESET);
}

/// Prints a green success banner with sparkle decorations.
pub fn print_success_banner(msg: &str) {
    use colors::{BOLD, MFB_GREEN, RESET};
    use symbols::SPARKLE;
    eprintln!("\n   {BOLD}{MFB_GREEN}{SPARKLE} {msg}  {SPARKLE}{RESET}{RESET}");
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::new();
    let mut in_escape = false;

    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Prints a stage heading with a diamond prefix and bold title.
pub fn print_stage(_icon: &str, title: &str) {
    use colors::{BOLD, MFB_BLUE, RESET};
    eprintln!(
        "{} {} {}{}{}",
        MFB_BLUE,
        symbols::DIAMOND,
        BOLD,
        title,
        RESET
    );
    let _ = io::stderr().flush();
}

/// Prints an indented sub-stage heading with a bullet prefix.
pub fn print_substage(title: &str) {
    use colors::RESET;
    eprintln!("   {} {} {}{}", colors::DIM, symbols::BULLET, RESET, title);
}

/// Prints a success message in bright green with a check indicator.
pub fn print_success(msg: &str) {
    use colors::{BRIGHT_GREEN, RESET};
    eprintln!("{}{} {}{}", BRIGHT_GREEN, symbols::SUCCESS, msg, RESET);
}

/// Prints a warning message in bright yellow with a warning indicator.
pub fn print_warning(msg: &str) {
    use colors::{BRIGHT_YELLOW, RESET};
    eprintln!("{}{} {}{}", BRIGHT_YELLOW, symbols::WARNING, msg, RESET);
}

/// Prints an error message in bright red with an error indicator.
pub fn print_error(msg: &str) {
    use colors::{BRIGHT_RED, RESET};
    eprintln!("{}{} {}{}", BRIGHT_RED, symbols::ERROR, msg, RESET);
}

/// Prints an informational message in bright cyan with an info indicator.
pub fn print_info(msg: &str) {
    use colors::{BRIGHT_CYAN, RESET};
    eprintln!("{}{} {}{}", BRIGHT_CYAN, symbols::INFO, msg, RESET);
}

/// Formats a byte count into a human-readable string with appropriate units (B, KB, MB, GB).
#[must_use]
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!(
            "{:.2} GB",
            crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(GB)
        )
    } else if bytes >= MB {
        format!(
            "{:.2} MB",
            crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(MB)
        )
    } else if bytes >= KB {
        format!(
            "{:.1} KB",
            crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(KB)
        )
    } else {
        format!("{bytes} B")
    }
}

/// Formats a duration in seconds into a human-readable string (e.g., "1h 01m 05s").
#[must_use]
pub fn format_duration(secs: f64) -> String {
    if secs >= 3600.0 {
        let h = crate::numeric_cast::f64_to_u32_sat((secs / 3600.0).floor());
        let m = crate::numeric_cast::f64_to_u32_sat(((secs % 3600.0) / 60.0).floor());
        let s = crate::numeric_cast::f64_to_u32_sat((secs % 60.0).floor());
        format!("{h}h {m:02}m {s:02}s")
    } else if secs >= 60.0 {
        let m = crate::numeric_cast::f64_to_u32_sat((secs / 60.0).floor());
        let s = crate::numeric_cast::f64_to_u32_sat((secs % 60.0).floor());
        format!("{m}m {s:02}s")
    } else {
        format!("{secs:.1}s")
    }
}

/// Formats a size-change percentage with color coding and decorative icons.
#[must_use]
pub fn format_size_change(pct: f64) -> String {
    use colors::{BRIGHT_GREEN, BRIGHT_RED, BRIGHT_YELLOW, RESET};

    if pct < -50.0 {
        format!("{}{:+.1}%{} {}", BRIGHT_GREEN, pct, RESET, symbols::SPARKLE)
    } else if pct < 0.0 {
        format!("{}{:+.1}%{} {}", BRIGHT_GREEN, pct, RESET, symbols::SAVE)
    } else if pct < 10.0 {
        format!("{BRIGHT_YELLOW}{pct:+.1}%{RESET}")
    } else {
        format!("{}{:+.1}%{} {}", BRIGHT_RED, pct, RESET, symbols::WARNING)
    }
}

/// Formats a signed byte difference into a human-readable string with a sign prefix.
#[must_use]
pub fn format_size_diff(diff_bytes: i64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    let abs_diff = diff_bytes.unsigned_abs();
    let sign = if diff_bytes >= 0 { "+" } else { "-" };

    if abs_diff >= MB {
        format!(
            "{}{:.1} MB",
            sign,
            crate::numeric_cast::u64_to_f64(abs_diff) / crate::numeric_cast::u64_to_f64(MB)
        )
    } else if abs_diff >= KB {
        format!(
            "{}{:.1} KB",
            sign,
            crate::numeric_cast::u64_to_f64(abs_diff) / crate::numeric_cast::u64_to_f64(KB)
        )
    } else {
        format!("{sign}{abs_diff} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar() {
        let bar = render_progress_bar(0.5, 20, ProgressStyle::Modern);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1500), "1.5 KB");
        assert_eq!(format_size(1_500_000), "1.43 MB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(5.5), "5.5s");
        assert_eq!(format_duration(65.0), "1m 05s");
        assert_eq!(format_duration(3665.0), "1h 01m 05s");
    }

    #[test]
    fn test_strip_ansi() {
        let s = "\x1b[31mRed\x1b[0m Text";
        assert_eq!(strip_ansi(s), "Red Text");
    }
}

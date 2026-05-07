//! Progress Bar Module
//!
//! Unified Progress Bar System:
//! - Project-wide unified style: ████████▓▓░░░░░░
//! - Thicker and more prominent progress bars
//! - Fixed display at the bottom of the terminal
//! - Detailed progress parameters (current file, ETA, speed, SSIM, CRF, etc.)
//!
//! Reference: media/CONTRIBUTING.md - Visual Progress Bar requirement

use crate::modern_ui::progress_style;
use crate::progress_mode::format_duration_compact;
use console::{measure_text_width, truncate_str};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::io::{self, Write};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

static PROGRESS_STDERR_LOCK: Mutex<()> = Mutex::new(());
static ACTIVE_PROGRESS_LINE: Mutex<Option<String>> = Mutex::new(None);
const SUB_SPINNER_TEMPLATE: &str = "  {spinner:.green} {prefix:.dim}: {msg}";

pub struct CoarseProgressBar {
    total: u64,
    current: AtomicU64,
    start_time: Instant,
    prefix: String,
    last_render: Arc<Mutex<Instant>>,
    message: Arc<Mutex<String>>,
    is_finished: AtomicBool,
    enabled: bool,
}

fn progress_line_enabled() -> bool {
    if std::env::var("FORCE_COLOR").is_ok() {
        return true;
    }
    console::Term::stderr().is_term()
}

fn terminal_columns() -> usize {
    if let Ok(cols) = std::env::var("COLUMNS")
        && let Ok(parsed) = cols.parse::<usize>()
        && parsed > 0
    {
        return parsed;
    }

    let (_, cols) = console::Term::stderr().size();
    cols as usize
}

fn truncate_progress_message(msg: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    if measure_text_width(msg) <= max_width {
        return msg.to_string();
    }

    truncate_str(msg, max_width, "…").into_owned()
}

fn dynamic_bar_width(terminal_width: usize, reserved_width: usize) -> usize {
    let remaining = terminal_width.saturating_sub(reserved_width);
    let preferred = remaining.min(progress_style::BAR_WIDTH);

    if terminal_width >= 120 {
        preferred.clamp(14, progress_style::BAR_WIDTH)
    } else if terminal_width >= 96 {
        preferred.clamp(10, 24)
    } else if terminal_width >= 72 {
        preferred.clamp(8, 16)
    } else {
        preferred.clamp(6, 10)
    }
}

#[derive(Debug, Clone, Copy)]
struct ProgressVariant {
    flags: u8,
}

const SHOW_BAR: u8 = 1;
const SHOW_ELAPSED: u8 = 2;
const SHOW_ETA: u8 = 4;
const SHOW_MESSAGE: u8 = 8;

impl ProgressVariant {
    const fn get_all(message_empty: bool) -> [Self; 5] {
        [
            Self {
                flags: SHOW_BAR
                    | SHOW_ELAPSED
                    | SHOW_ETA
                    | if message_empty { 0 } else { SHOW_MESSAGE },
            },
            Self {
                flags: SHOW_BAR | SHOW_ELAPSED | SHOW_ETA,
            },
            Self {
                flags: SHOW_BAR | SHOW_ELAPSED,
            },
            Self { flags: SHOW_BAR },
            Self { flags: 0 },
        ]
    }

    const fn show_bar(self) -> bool {
        (self.flags & SHOW_BAR) != 0
    }

    const fn show_elapsed(self) -> bool {
        (self.flags & SHOW_ELAPSED) != 0
    }

    const fn show_eta(self) -> bool {
        (self.flags & SHOW_ETA) != 0
    }

    const fn show_message(self) -> bool {
        (self.flags & SHOW_MESSAGE) != 0
    }
}

fn calculate_fixed_width(variant: ProgressVariant, widths: &LayoutWidths) -> usize {
    let mut fixed_width = widths.prefix + 1;
    if !variant.show_bar() {
        fixed_width += widths.percent + 3;
    }
    fixed_width += widths.counts;

    if variant.show_elapsed() {
        fixed_width += 3 + measure_text_width("⏱️ ") + widths.elapsed;
    }
    if variant.show_eta() {
        fixed_width += 3 + measure_text_width("ETA ") + widths.eta_val;
    }
    if widths.stats > 0 {
        fixed_width += widths.stats;
    }

    if variant.show_bar() {
        fixed_width += 1 + widths.bar_left + widths.bar_right + 1;
    }
    fixed_width
}

struct LayoutWidths {
    prefix: usize,
    percent: usize,
    counts: usize,
    elapsed: usize,
    eta_val: usize,
    stats: usize,
    bar_left: usize,
    bar_right: usize,
}

#[derive(Clone, Copy)]
struct AssembleContext<'a> {
    prefix: &'a str,
    variant: ProgressVariant,
    percent: f64,
    bar_width: usize,
    percent_str: &'a str,
    counts_str: &'a str,
    elapsed_str: &'a str,
    eta_str: &'a str,
    message_text: &'a str,
    stats: &'a str,
    terminal_width: usize,
}

fn build_coarse_progress_line(
    prefix: &str,
    percent: f64,
    current: u64,
    total: u64,
    elapsed: Duration,
    eta_str: &str,
    message: &str,
    stats: &str,
    terminal_width: usize,
) -> String {
    let color = "\x1b[32m";
    let percent_str = format!("{percent:>5.1}%");
    let counts_str = format!("{current}/{total}");
    let elapsed_str = format_duration_compact(elapsed);

    let widths = LayoutWidths {
        prefix: measure_text_width(prefix),
        percent: measure_text_width(&percent_str),
        counts: measure_text_width(&counts_str),
        elapsed: measure_text_width(&elapsed_str),
        eta_val: measure_text_width(eta_str),
        stats: measure_text_width(stats),
        bar_left: measure_text_width(progress_style::BAR_LEFT),
        bar_right: measure_text_width(progress_style::BAR_RIGHT),
    };

    for variant in ProgressVariant::get_all(message.is_empty()) {
        let fixed_width = calculate_fixed_width(variant, &widths);
        let min_bar = if variant.show_bar() { 6 } else { 0 };
        let mut bar_width = if variant.show_bar() {
            dynamic_bar_width(terminal_width, fixed_width)
        } else {
            0
        };

        if variant.show_bar() && bar_width < min_bar {
            continue;
        }

        let message_text = if variant.show_message() {
            let available = terminal_width.saturating_sub(fixed_width + bar_width + 3);
            if available < 6 {
                if variant.show_bar() && bar_width > min_bar {
                    let needed = 6_usize.saturating_sub(available);
                    if bar_width.saturating_sub(needed) >= min_bar {
                        bar_width -= needed;
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            truncate_progress_message(message, available.clamp(6, 24))
        } else {
            String::new()
        };

        let final_fixed =
            fixed_width + message_text.len() + (if message_text.is_empty() { 0 } else { 3 });
        if variant.show_bar() && bar_width + final_fixed > terminal_width {
            bar_width = terminal_width.saturating_sub(final_fixed + 1).max(min_bar);
            if bar_width < min_bar {
                continue;
            }
        }

        let line = assemble_coarse_line(AssembleContext {
            prefix,
            variant,
            percent,
            bar_width,
            percent_str: &percent_str,
            counts_str: &counts_str,
            elapsed_str: &elapsed_str,
            eta_str,
            message_text: &message_text,
            stats,
            terminal_width,
        });

        if let Some(l) = line {
            return l;
        }
    }

    fallback_coarse_line(color, prefix, percent, &percent_str, &counts_str, stats)
}

fn assemble_coarse_line(ctx: AssembleContext<'_>) -> Option<String> {
    let color = "\x1b[32m";
    let mut line = String::with_capacity(ctx.terminal_width + 32);
    line.push_str(color);
    line.push_str(ctx.prefix);
    line.push(' ');

    if ctx.variant.show_bar() {
        let filled = crate::numeric_cast::f64_to_usize_sat(
            ((ctx.percent / 100.0) * crate::numeric_cast::usize_to_f64(ctx.bar_width)).round(),
        );
        let empty = ctx.bar_width.saturating_sub(filled);
        line.push_str(progress_style::BAR_LEFT);
        for _ in 0..filled {
            line.push('█');
        }
        for _ in 0..empty {
            line.push('░');
        }
        line.push_str(progress_style::BAR_RIGHT);
        line.push(' ');
    }

    if !ctx.variant.show_bar() {
        line.push_str(ctx.percent_str);
        line.push_str(" • ");
    }
    line.push_str(ctx.counts_str);

    if ctx.variant.show_elapsed() {
        line.push_str(" • ⏱️ ");
        line.push_str(ctx.elapsed_str);
    }
    if ctx.variant.show_eta() {
        line.push_str(" • ETA ");
        line.push_str(ctx.eta_str);
    }
    if !ctx.message_text.is_empty() {
        line.push_str(" • ");
        line.push_str(ctx.message_text);
    }

    line.push_str("\x1b[0m");
    line.push_str(ctx.stats);

    if measure_text_width(&line) <= ctx.terminal_width.saturating_sub(1).max(32) {
        Some(line)
    } else {
        None
    }
}

fn fallback_coarse_line(
    color: &str,
    prefix: &str,
    percent: f64,
    percent_str: &str,
    counts_str: &str,
    stats: &str,
) -> String {
    let mut final_line = format!("{color}{prefix} ");
    {
        use std::fmt::Write;
        if percent < 100.0_f64 {
            write!(final_line, "{percent_str} • ").expect("String formatting should not fail");
        }
        write!(final_line, "{counts_str}\x1b[0m{stats}")
            .expect("String formatting should not fail");
    }
    final_line
}

fn build_finished_progress_line(
    prefix: &str,
    total: u64,
    elapsed: Duration,
    stats: &str,
    terminal_width: usize,
) -> String {
    build_coarse_progress_line(
        prefix,
        100.0,
        total,
        total.max(1),
        elapsed,
        "---",
        "done",
        stats,
        terminal_width,
    )
}

fn set_active_progress_line(line: Option<String>) {
    if let Ok(mut guard) = ACTIVE_PROGRESS_LINE.lock() {
        *guard = line;
    }
}

pub fn active_progress_line() -> Option<String> {
    ACTIVE_PROGRESS_LINE
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
}

#[must_use]
pub fn wrap_output_for_active_progress(line: &str) -> String {
    active_progress_line().map_or_else(
        || format!("{line}\n"),
        |progress_line| {
            // 1. \r: move to start
            // 2. \x1b[2K: clear entire line
            // 3. line + \n: print the actual log message and go to next line
            // 4. \r + progress_line: print progress bar at the new start
            format!("\r\x1b[2K{line}\n\r{progress_line}")
        },
    )
}

impl CoarseProgressBar {
    pub fn new(total: u64, prefix: &str) -> Self {
        let enabled = progress_line_enabled();
        if enabled && let Ok(_guard) = PROGRESS_STDERR_LOCK.lock() {
            eprint!("\x1b[?25l");
            let _ = io::stderr().flush();
        }

        Self {
            total,
            current: AtomicU64::new(0),
            start_time: Instant::now(),
            prefix: prefix.to_string(),
            last_render: Arc::new(Mutex::new(Instant::now())),
            message: Arc::new(Mutex::new(String::new())),
            is_finished: AtomicBool::new(false),
            enabled,
        }
    }

    pub fn set(&self, current: u64) {
        self.current.store(current, Ordering::Relaxed);

        if !self.enabled {
            return;
        }

        if let Ok(mut last) = self.last_render.try_lock()
            && last.elapsed() >= Duration::from_millis(33)
        {
            self.render();
            *last = Instant::now();
        }
    }

    pub fn inc(&self) {
        let current = self.current.fetch_add(1, Ordering::Relaxed) + 1;
        if current.is_multiple_of(10) {
            self.set(current);
        }
    }

    pub fn set_message(&self, msg: &str) {
        if let Ok(mut current_message) = self.message.lock() {
            *current_message = msg.to_string();
        }
        self.set(self.current.load(Ordering::Relaxed));
    }

    pub fn println(&self, msg: &str) {
        if self.is_finished.load(Ordering::Relaxed) {
            eprintln!("{msg}");
            return;
        }

        if let Ok(_guard) = PROGRESS_STDERR_LOCK.lock() {
            eprint!("\r\x1b[K");
            let _ = io::stderr().flush();

            eprintln!("{msg}");
        }

        if self.enabled {
            self.render();
        }
    }

    fn render(&self) {
        if self.is_finished.load(Ordering::Relaxed) || !self.enabled {
            return;
        }

        let current = self.current.load(Ordering::Relaxed);
        let total = self.total.max(1);
        let percent = (crate::numeric_cast::u64_to_f64(current)
            / crate::numeric_cast::u64_to_f64(total)
            * 100.0)
            .min(100.0);
        let elapsed = self.start_time.elapsed();
        let message = self
            .message
            .lock()
            .map(|msg| msg.clone())
            .unwrap_or_default();
        let stats = crate::progress_mode::get_current_stats_string();

        let eta_str = if current > 0 && current < total {
            let avg_time = elapsed.as_secs_f64() / crate::numeric_cast::u64_to_f64(current);
            let remaining_secs = crate::numeric_cast::f64_to_u64_sat(
                crate::numeric_cast::u64_to_f64(total - current) * avg_time,
            );
            format_eta_simple(remaining_secs)
        } else {
            "---".to_string()
        };
        let terminal_width = terminal_columns().max(48);
        let line = build_coarse_progress_line(
            &self.prefix,
            percent,
            current,
            total,
            elapsed,
            &eta_str,
            &message,
            &stats,
            terminal_width,
        );

        if let Ok(_guard) = PROGRESS_STDERR_LOCK.lock() {
            set_active_progress_line(None);
            eprint!("\r\x1b[K{line}");
            set_active_progress_line(Some(line));
            let _ = io::stderr().flush();
        }
    }

    pub fn finish(&self) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }

        let total = self.total;
        let elapsed = self.start_time.elapsed();

        let stats = crate::progress_mode::get_current_stats_string();
        let terminal_width = terminal_columns().max(48);
        let line =
            build_finished_progress_line(&self.prefix, total, elapsed, &stats, terminal_width);

        if let Ok(_guard) = PROGRESS_STDERR_LOCK.lock() {
            set_active_progress_line(None);
            eprint!("\r\x1b[K{line}\n");

            eprint!("\x1b[?25h");
            let _ = io::stderr().flush();
        }
    }

    pub fn finish_and_clear(&self) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }

        if self.enabled
            && let Ok(_guard) = PROGRESS_STDERR_LOCK.lock()
        {
            set_active_progress_line(None);
            eprint!("\r\x1b[K");
            eprint!("\x1b[?25h");
            let _ = io::stderr().flush();
        }
    }
}

impl Drop for CoarseProgressBar {
    fn drop(&mut self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            self.finish();
        }
    }
}

pub struct DetailedCoarseProgressBar {
    prefix: String,
    total_iterations: u64,
    current_iteration: AtomicU64,
    input_size: u64,
    current_crf: AtomicU32,
    current_size: AtomicU64,
    /// SSIM bits; meaning depends on `has_ssim` (0.0 is a valid value).
    current_ssim: AtomicU64,
    has_ssim: AtomicBool,
    best_crf: AtomicU32,
    start_time: Instant,
    last_render: Arc<Mutex<Instant>>,
    is_finished: AtomicBool,
}

impl DetailedCoarseProgressBar {
    #[must_use]
    pub fn new(prefix: &str, input_size: u64, total_iterations: u64) -> Self {
        eprint!("\x1b[?25l");
        let _ = io::stderr().flush();

        Self {
            prefix: prefix.to_string(),
            total_iterations,
            current_iteration: AtomicU64::new(0),
            input_size,
            current_crf: AtomicU32::new(0),
            current_size: AtomicU64::new(0),
            current_ssim: AtomicU64::new(0),
            has_ssim: AtomicBool::new(false),
            best_crf: AtomicU32::new(0),
            start_time: Instant::now(),
            last_render: Arc::new(Mutex::new(Instant::now())),
            is_finished: AtomicBool::new(false),
        }
    }

    pub fn inc_iteration(&self, crf: f32, size: u64, ssim: Option<f64>) {
        let iter = self.current_iteration.fetch_add(1, Ordering::Relaxed) + 1;

        self.current_crf.store(crf.to_bits(), Ordering::Relaxed);
        self.current_size.store(size, Ordering::Relaxed);
        if let Some(s) = ssim {
            self.current_ssim.store(s.to_bits(), Ordering::Relaxed);
            self.has_ssim.store(true, Ordering::Relaxed);
        }

        if size < self.input_size {
            self.best_crf.store(crf.to_bits(), Ordering::Relaxed);
        }

        self.render(iter, crf, size, ssim);
    }

    fn render(&self, iter: u64, crf: f32, size: u64, ssim: Option<f64>) {
        if self.is_finished.load(Ordering::Relaxed) {
            return;
        }

        if let Ok(mut last) = self.last_render.try_lock() {
            if last.elapsed() < Duration::from_millis(33) {
                return;
            }
            *last = Instant::now();
        } else {
            return;
        }

        let total = self.total_iterations.max(1);
        let percent = (crate::numeric_cast::u64_to_f64(iter)
            / crate::numeric_cast::u64_to_f64(total)
            * 100.0)
            .min(100.0);
        let elapsed = self.start_time.elapsed();

        let size_pct = if self.input_size > 0 {
            ((crate::numeric_cast::u64_to_f64(size)
                / crate::numeric_cast::u64_to_f64(self.input_size))
                - 1.0_f64)
                * 100.0_f64
        } else {
            0.0_f64
        };

        let icon = if size < self.input_size {
            "💾"
        } else {
            "📈"
        };

        let ssim_str = ssim.map_or_else(String::new, |s| format!("SSIM {s:.4}"));

        let best_crf = f32::from_bits(self.best_crf.load(Ordering::Relaxed));
        let best_str = if best_crf > 0.0 {
            format!("Best: {best_crf:.1}")
        } else {
            String::new()
        };

        let terminal_width = terminal_columns().max(48);
        let prefix_width = measure_text_width(&self.prefix);
        let reserved = if ssim_str.is_empty() { 65 } else { 78 };

        let bar_width = dynamic_bar_width(terminal_width, reserved + prefix_width);
        // Ensure we don't overflow the subtraction
        let available_for_prefix = terminal_width.saturating_sub(reserved + bar_width);
        let filled = crate::numeric_cast::f64_to_usize_sat(
            ((percent / 100.0) * crate::numeric_cast::usize_to_f64(bar_width)).round(),
        );
        let empty = bar_width.saturating_sub(filled);
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

        let color = "\x1b[32m";
        let line = format!(
            "{color}{prefix} {bar_left}{color}{bar}{color}▏ CRF {crf:.1} • {size:+.1}% {icon} • {ssim} • {best} • {iter}/{total} • ⏱️ {elapsed:.1}s\x1b[0m",
            color = color,
            prefix = truncate_progress_message(&self.prefix, available_for_prefix),
            bar_left = progress_style::BAR_LEFT,
            bar = bar,
            crf = crf,
            size = size_pct,
            icon = icon,
            ssim = ssim_str,
            best = best_str,
            iter = iter,
            total = total,
            elapsed = elapsed.as_secs_f64()
        );

        if let Ok(_guard) = PROGRESS_STDERR_LOCK.lock() {
            set_active_progress_line(None);
            eprint!("\r\x1b[K{line}");
            set_active_progress_line(Some(line));
            let _ = io::stderr().flush();
        }
    }

    /// Print a message to the terminal without interfering with the progress bar.
    ///
    /// # Panics
    /// Panics if the internal clock is invalid (e.g. `ImageMagick`'s internal property interpretation).
    pub fn println(&self, msg: &str) {
        if self.is_finished.load(Ordering::Relaxed) {
            eprintln!("{msg}");
            return;
        }

        if let Ok(_guard) = PROGRESS_STDERR_LOCK.lock() {
            set_active_progress_line(None);
            eprint!("\r\x1b[K");
            let _ = io::stderr().flush();
        }

        eprintln!("{msg}");

        let iter = self.current_iteration.load(Ordering::Relaxed);
        let crf = f32::from_bits(self.current_crf.load(Ordering::Relaxed));
        let size = self.current_size.load(Ordering::Relaxed);
        let ssim = if self.has_ssim.load(Ordering::Relaxed) {
            Some(f64::from_bits(self.current_ssim.load(Ordering::Relaxed)))
        } else {
            None
        };

        if let Ok(mut last) = self.last_render.lock() {
            let now = Instant::now();
            *last = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
        }
        self.render(iter, crf, size, ssim);
    }

    pub fn finish(&self, final_crf: f32, final_size: u64, final_ssim: Option<f64>) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }

        let elapsed = self.start_time.elapsed();

        let size_pct = if self.input_size > 0 {
            ((crate::numeric_cast::u64_to_f64(final_size)
                / crate::numeric_cast::u64_to_f64(self.input_size))
                - 1.0_f64)
                * 100.0_f64
        } else {
            0.0_f64
        };

        let ssim_str = final_ssim
            .map(|s| format!("SSIM {s:.4}"))
            .unwrap_or_default();

        let icon = if size_pct < 0.0_f64 { "✅" } else { "⚠️" };
        let iter = self.current_iteration.load(Ordering::Relaxed);

        let bar_width: usize = progress_style::BAR_WIDTH;
        let bar = "█".repeat(bar_width);
        let color = "\x1b[32m";

        eprint!(
            "\r\x1b[K{color}{prefix} {bar_left}{color}{bar}{color}▏ ✅ 100% • CRF {final_crf:.1} • {size_pct:+.1}% {icon} • {ssim} • {iter} iterations • ⏱️ {elapsed:.1}s\x1b[0m\n",
            color = color,
            prefix = self.prefix,
            bar_left = progress_style::BAR_LEFT,
            bar = bar,
            final_crf = final_crf,
            size_pct = size_pct,
            icon = icon,
            ssim = ssim_str,
            iter = iter,
            elapsed = elapsed.as_secs_f64()
        );

        eprint!("\x1b[?25h");
        let _ = io::stderr().flush();
    }

    pub fn fail(&self, error: &str) {
        if self.is_finished.swap(true, Ordering::Relaxed) {
            return;
        }

        eprint!("\r\x1b[K❌ {} {}\n", self.prefix, error);
        eprint!("\x1b[?25h");
        let _ = io::stderr().flush();
    }
}

impl Drop for DetailedCoarseProgressBar {
    fn drop(&mut self) {
        if !self.is_finished.load(Ordering::Relaxed) {
            eprint!("\r\x1b[K");
            eprint!("\x1b[?25h");
            let _ = io::stderr().flush();
        }
    }
}

fn format_eta_simple(seconds: u64) -> String {
    if seconds > 86400 {
        return ">1d".to_string();
    }
    if seconds >= 3600 {
        format!("{}h{}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m{}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

pub struct FixedBottomProgress {
    bar: ProgressBar,
    start_time: Instant,
    total: u64,
    processed: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    skipped: AtomicU64,
    input_bytes: AtomicU64,
    output_bytes: AtomicU64,
    current_file: Arc<Mutex<String>>,
    current_stage: Arc<Mutex<String>>,
}

impl FixedBottomProgress {
    /// Create a new fixed-bottom progress bar for batch processing.
    ///
    /// # Panics
    ///
    /// Panics if the progress bar template is invalid.
    #[must_use]
    pub fn new(total: u64, prefix: &str) -> Self {
        let bar = ProgressBar::new(total);

        bar.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::BATCH_TEMPLATE)
                .expect("Invalid progress bar template")
                .progress_chars(progress_style::PROGRESS_CHARS)
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        bar.set_prefix(prefix.to_string());
        bar.enable_steady_tick(Duration::from_millis(33));

        bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(30));

        Self {
            bar,
            start_time: Instant::now(),
            total,
            processed: AtomicU64::new(0),
            succeeded: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            input_bytes: AtomicU64::new(0),
            output_bytes: AtomicU64::new(0),
            current_file: Arc::new(Mutex::new(String::new())),
            current_stage: Arc::new(Mutex::new(String::new())),
        }
    }

    pub fn set_current_file(&self, filename: &str) {
        if let Ok(mut f) = self.current_file.lock() {
            *f = filename.to_string();
        }
        self.update_message();
    }

    pub fn set_stage(&self, stage: &str) {
        if let Ok(mut s) = self.current_stage.lock() {
            *s = stage.to_string();
        }
        self.update_message();
    }

    fn update_message(&self) {
        let file = self
            .current_file
            .lock()
            .map(|f| f.clone())
            .unwrap_or_default();
        let stage = self
            .current_stage
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();

        let msg = if !stage.is_empty() && !file.is_empty() {
            format!("{} | {}", stage, truncate_filename(&file, 40))
        } else if !file.is_empty() {
            truncate_filename(&file, 50)
        } else if !stage.is_empty() {
            stage
        } else {
            "Processing...".to_string()
        };

        self.bar.set_message(msg);
    }

    pub fn success(&self, input_size: u64, output_size: u64) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.succeeded.fetch_add(1, Ordering::Relaxed);
        self.input_bytes.fetch_add(input_size, Ordering::Relaxed);
        self.output_bytes.fetch_add(output_size, Ordering::Relaxed);
        self.bar.inc(1);
    }

    pub fn fail(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.failed.fetch_add(1, Ordering::Relaxed);
        self.bar.inc(1);
    }

    pub fn skip(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
        self.skipped.fetch_add(1, Ordering::Relaxed);
        self.bar.inc(1);
    }

    pub fn stats(&self) -> ProgressStats {
        let input = self.input_bytes.load(Ordering::Relaxed);
        let output = self.output_bytes.load(Ordering::Relaxed);
        ProgressStats {
            total: self.total,
            processed: self.processed.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            input_bytes: input,
            output_bytes: output,
            elapsed: self.start_time.elapsed(),
            compression_ratio: if input > 0 {
                crate::numeric_cast::u64_to_f64(output) / crate::numeric_cast::u64_to_f64(input)
            } else {
                1.0
            },
        }
    }

    pub fn finish(&self) {
        let stats = self.stats();
        let saved = stats.input_bytes.saturating_sub(stats.output_bytes);

        self.bar.finish_with_message(format!(
            "✅ {} succeeded, {} failed, {} skipped | Saved: {}",
            stats.succeeded,
            stats.failed,
            stats.skipped,
            format_bytes(saved)
        ));
    }

    pub const fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

#[derive(Debug, Clone)]
pub struct ProgressStats {
    pub total: u64,
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub skipped: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub elapsed: Duration,
    pub compression_ratio: f64,
}

pub struct ExploreProgress {
    start_time: Instant,
    input_size: u64,
    current_crf: Arc<Mutex<f32>>,
    current_ssim: Arc<Mutex<Option<f64>>>,
    current_size: Arc<Mutex<u64>>,
    iterations: AtomicUsize,
    stage: Arc<Mutex<String>>,
    best_crf: Arc<Mutex<f32>>,
    best_ssim: Arc<Mutex<f64>>,
}

impl ExploreProgress {
    #[must_use]
    pub fn new(input_size: u64) -> Self {
        Self {
            start_time: Instant::now(),
            input_size,
            current_crf: Arc::new(Mutex::new(0.0)),
            current_ssim: Arc::new(Mutex::new(None)),
            current_size: Arc::new(Mutex::new(0)),
            iterations: AtomicUsize::new(0),
            stage: Arc::new(Mutex::new("Initializing".to_string())),
            best_crf: Arc::new(Mutex::new(0.0)),
            best_ssim: Arc::new(Mutex::new(0.0)),
        }
    }

    pub fn update_crf(&self, crf: f32, size: u64, ssim: Option<f64>) {
        if let Ok(mut c) = self.current_crf.lock() {
            *c = crf;
        }
        if let Ok(mut s) = self.current_size.lock() {
            *s = size;
        }
        if let Ok(mut ss) = self.current_ssim.lock() {
            *ss = ssim;
        }
        self.iterations.fetch_add(1, Ordering::Relaxed);
        self.print_status();
    }

    pub fn set_stage(&self, stage: &str) {
        if let Ok(mut s) = self.stage.lock() {
            *s = stage.to_string();
        }
        self.print_status();
    }

    pub fn update_best(&self, crf: f32, ssim: f64) {
        if let Ok(mut c) = self.best_crf.lock() {
            *c = crf;
        }
        if let Ok(mut s) = self.best_ssim.lock() {
            *s = ssim;
        }
    }

    fn print_status(&self) {
        let crf = self.current_crf.lock().map_or(0.0, |c| *c);
        let size = self.current_size.lock().map_or(0, |s| *s);
        let ssim = self.current_ssim.lock().ok().and_then(|s| *s);
        let stage = self.stage.lock().map(|s| s.clone()).unwrap_or_default();
        let iter = self.iterations.load(Ordering::Relaxed);
        let best_crf = self.best_crf.lock().map_or(0.0, |c| *c);
        let best_ssim = self.best_ssim.lock().map_or(0.0_f64, |s| *s);

        let size_change = if self.input_size > 0 {
            ((crate::numeric_cast::u64_to_f64(size)
                / crate::numeric_cast::u64_to_f64(self.input_size))
                - 1.0_f64)
                * 100.0_f64
        } else {
            0.0_f64
        };

        let elapsed = self.start_time.elapsed();
        let ssim_str = ssim.map_or_else(|| "---".to_string(), |s| format!("{s:.4}"));
        let compress_icon = if size < self.input_size { "✅" } else { "❌" };

        let line = format!(
            "🔍 Explore: {} • CRF {:.1} • SSIM {} • Size {:+.1}% {} • Iter {} • Best: CRF {:.1} / SSIM {:.4} • ⏱️ {:.1}s",
            stage,
            crf,
            ssim_str,
            size_change,
            compress_icon,
            iter,
            best_crf,
            best_ssim,
            elapsed.as_secs_f64()
        );

        if let Ok(_guard) = PROGRESS_STDERR_LOCK.lock() {
            set_active_progress_line(None);
            eprint!("\r\x1b[K{line}");
            set_active_progress_line(Some(line));
            let _ = io::stderr().flush();
        }
    }

    pub fn finish(&self, result_crf: f32, result_ssim: f64, result_size: u64) {
        let size_change = if self.input_size > 0 {
            ((crate::numeric_cast::u64_to_f64(result_size)
                / crate::numeric_cast::u64_to_f64(self.input_size))
                - 1.0_f64)
                * 100.0_f64
        } else {
            0.0_f64
        };
        let elapsed = self.start_time.elapsed();
        let iter = self.iterations.load(Ordering::Relaxed);

        if let Ok(_guard) = PROGRESS_STDERR_LOCK.lock() {
            set_active_progress_line(None);
            eprint!("\r\x1b[K");
            eprintln!(
                "✅ Explore Done: CRF {:.1} • SSIM {:.4} • Size {:+.1}% • {} iter in {:.1}s",
                result_crf,
                result_ssim,
                size_change,
                iter,
                elapsed.as_secs_f64()
            );
            let _ = io::stderr().flush();
        }
    }
}

pub struct ExploreLogger {
    input_size: u64,
    start_time: Instant,
    iterations: usize,
    best_crf: f32,
    best_ssim: f64,
    best_size: u64,
    show_progress_bar: bool,
}

impl ExploreLogger {
    #[must_use]
    pub fn new(input_size: u64, show_progress_bar: bool) -> Self {
        Self {
            input_size,
            start_time: Instant::now(),
            iterations: 0,
            best_crf: 0.0,
            best_ssim: 0.0,
            best_size: 0,
            show_progress_bar,
        }
    }

    pub fn stage(&mut self, name: &str) {
        if self.show_progress_bar {
            eprintln!("\n   📍 {name}");
        }
    }

    pub fn test(&mut self, crf: f32, size: u64, ssim: Option<f64>) {
        self.iterations += 1;
        let size_change = self.calc_change(size);
        let compress_ok = size < self.input_size;

        if self.show_progress_bar {
            let ssim_str = ssim.map(|s| format!("SSIM {s:.4}")).unwrap_or_default();
            let icon = if compress_ok { "✅" } else { "❌" };
            eprint!("\r\x1b[K   🔄 CRF {crf:.1}: {size_change:+.1}% {icon} {ssim_str}");
            let _ = io::stderr().flush();
        }
    }

    pub fn new_best(&mut self, crf: f32, size: u64, ssim: f64) {
        self.best_crf = crf;
        self.best_size = size;
        self.best_ssim = ssim;

        if self.show_progress_bar {
            eprintln!(" ← 🎯 New best!");
        }
    }

    pub fn direction(&self, msg: &str) {
        if self.show_progress_bar {
            eprintln!("\r\x1b[K      {msg}");
        }
    }

    pub fn early_stop(&self, reason: &str) {
        if self.show_progress_bar {
            eprintln!("\r\x1b[K   ⚡ Early stop: {reason}");
        }
    }

    fn calc_change(&self, size: u64) -> f64 {
        if self.input_size > 0 {
            ((crate::numeric_cast::u64_to_f64(size)
                / crate::numeric_cast::u64_to_f64(self.input_size))
                - 1.0)
                * 100.0
        } else {
            0.0
        }
    }

    pub fn finish(&self) {
        if !self.show_progress_bar {
            return;
        }

        let elapsed = self.start_time.elapsed();
        let size_change = self.calc_change(self.best_size);
        let saved = self.input_size.saturating_sub(self.best_size);

        eprintln!("\r\x1b[K");
        eprintln!("   ═══════════════════════════════════════════════════");
        eprintln!(
            "   📊 Result: CRF {:.1} | SSIM {:.4} | {:+.1}%",
            self.best_crf, self.best_ssim, size_change
        );
        if saved > 0 {
            eprintln!(
                "   💾 Saved: {} ({:.2} MB)",
                format_bytes(saved),
                crate::numeric_cast::u64_to_f64(saved) / 1_024.0_f64 / 1_024.0_f64
            );
        }
        eprintln!(
            "   📈 Iterations: {} | Time: {:.1}s",
            self.iterations,
            elapsed.as_secs_f64()
        );
    }
}

/// Create a professional-looking spinner.
///
/// # Panics
/// Panics if the spinner template is invalid.
#[must_use]
pub fn create_professional_spinner(prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();

    if crate::progress_mode::is_quiet_mode() {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        pb.set_style(
            ProgressStyle::default_spinner()
                .template(progress_style::SPINNER_TEMPLATE)
                .expect("Invalid spinner template")
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(Duration::from_millis(8));
        pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
    }
    pb
}

/// Create a standard progress bar.
///
/// # Panics
/// Panics if the progress bar template is invalid.
#[must_use]
pub fn create_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);

    if crate::progress_mode::is_quiet_mode() {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        // Use a custom template that doesn't include elapsed time
        // We'll handle time formatting in the progress updates
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} {prefix:.cyan.bold} ▕{bar:35.green/black}▏ {percent:>3}% • {pos}/{len} • {msg}")
                .expect("Invalid progress bar template")
                .progress_chars(progress_style::PROGRESS_CHARS)
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(Duration::from_millis(8));
        pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
    }
    pb
}

/// Create a detailed progress bar for batch operations.
///
/// # Panics
/// Panics if the batch progress bar template is invalid.
#[must_use]
pub fn create_detailed_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);

    if crate::progress_mode::is_quiet_mode() {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        pb.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::BATCH_TEMPLATE)
                .expect("Invalid progress bar template")
                .progress_chars(progress_style::PROGRESS_CHARS)
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(Duration::from_millis(8));
        pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
    }
    pb
}

/// Create a compact progress bar.
///
/// # Panics
/// Panics if the compact progress bar template is invalid.
#[must_use]
pub fn create_compact_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);

    if crate::progress_mode::is_quiet_mode() {
        pb.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        pb.set_style(
            ProgressStyle::default_bar()
                .template(progress_style::COMPACT_TEMPLATE)
                .expect("Invalid progress bar template")
                .progress_chars(progress_style::PROGRESS_CHARS),
        );
        pb.set_prefix(prefix.to_string());
        pb.enable_steady_tick(Duration::from_millis(8));
        pb.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
    }
    pb
}

#[must_use]
pub fn create_progress_bar_with_eta(total: u64, prefix: &str) -> SmartProgressBar {
    SmartProgressBar::new(total, prefix)
}

pub struct SmartProgressBar {
    bar: ProgressBar,
    start_time: Instant,
    total: u64,
    processed: u64,
    recent_times: Vec<f64>,
    last_update: Instant,
}

impl SmartProgressBar {
    /// Create a new `SmartProgressBar`.
    ///
    /// # Panics
    /// Panics if the progress bar template is invalid.
    #[must_use]
    pub fn new(total: u64, prefix: &str) -> Self {
        let bar = ProgressBar::new(total);

        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(progress_style::BATCH_TEMPLATE)
                    .expect("Invalid progress bar template")
                    .progress_chars(progress_style::PROGRESS_CHARS)
                    .tick_chars(progress_style::SPINNER_CHARS),
            );
            bar.set_prefix(prefix.to_string());
            bar.enable_steady_tick(Duration::from_millis(8));
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
        }

        Self {
            bar,
            start_time: Instant::now(),
            total,
            processed: 0,
            recent_times: Vec::with_capacity(10),
            last_update: Instant::now(),
        }
    }

    pub fn inc(&mut self, message: &str) {
        let elapsed = self.last_update.elapsed().as_secs_f64();
        self.last_update = Instant::now();

        if self.recent_times.len() >= 10 {
            self.recent_times.remove(0);
        }
        self.recent_times.push(elapsed);

        self.processed += 1;
        self.bar.inc(1);

        let remaining = self.total.saturating_sub(self.processed);
        let eta = if !self.recent_times.is_empty() && remaining > 0 {
            let avg_time: f64 = self.recent_times.iter().sum::<f64>()
                / crate::numeric_cast::usize_to_f64(self.recent_times.len());
            let eta_secs = avg_time * crate::numeric_cast::u64_to_f64(remaining);
            format_eta(eta_secs)
        } else {
            "calculating...".to_string()
        };

        self.bar.set_message(format!("{eta} | {message}"));
    }

    pub fn finish(&self) {
        let total_time = self.start_time.elapsed();
        self.bar
            .finish_with_message(format!("Done in {}", format_duration(total_time)));
    }

    #[must_use]
    pub const fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

fn format_eta(seconds: f64) -> String {
    if seconds.is_nan() || seconds.is_infinite() || seconds < 0.0_f64 {
        return "unknown".to_string();
    }

    let secs = crate::numeric_cast::f64_to_u64_sat(seconds);

    if secs > 86400 {
        return ">24h".to_string();
    }

    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Create a simple spinner with a message.
///
/// # Panics
/// Panics if the spinner template is invalid.
#[must_use]
pub fn create_spinner(message: &str) -> ProgressBar {
    let spinner = ProgressBar::new_spinner();

    if crate::progress_mode::is_quiet_mode() {
        spinner.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("Invalid spinner template")
                .tick_chars(progress_style::SPINNER_CHARS),
        );
        spinner.set_message(message.to_string());
        spinner.enable_steady_tick(Duration::from_millis(8));
        spinner.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
    }
    spinner
}

#[must_use]
pub fn create_multi_progress() -> MultiProgress {
    MultiProgress::new()
}

pub struct BatchProgress {
    pub total: u64,
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub skipped: u64,
    bar: ProgressBar,
}

impl BatchProgress {
    #[must_use]
    pub fn new(total: u64, prefix: &str) -> Self {
        Self {
            total,
            processed: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            bar: create_progress_bar(total, prefix),
        }
    }

    pub fn success(&mut self, message: &str) {
        self.processed += 1;
        self.succeeded += 1;
        self.bar.set_message(format!("✅ {message}"));
        self.bar.inc(1);
    }

    pub fn fail(&mut self, message: &str) {
        self.processed += 1;
        self.failed += 1;
        self.bar.set_message(format!("❌ {message}"));
        self.bar.inc(1);
    }

    pub fn skip(&mut self, message: &str) {
        self.processed += 1;
        self.skipped += 1;
        self.bar.set_message(format!("⏭️  {message}"));
        self.bar.inc(1);
    }

    pub fn finish(&self) {
        self.bar.finish_with_message(format!(
            "Complete: {} succeeded, {} failed, {} skipped",
            self.succeeded, self.failed, self.skipped
        ));
    }

    #[must_use]
    pub const fn bar(&self) -> &ProgressBar {
        &self.bar
    }
}

fn truncate_filename(filename: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    if measure_text_width(filename) <= max_len {
        filename.to_string()
    } else {
        truncate_str(filename, max_len, "…").into_owned()
    }
}

#[must_use]
pub fn format_bytes(bytes: u64) -> String {
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
            "{:.2} KB",
            crate::numeric_cast::u64_to_f64(bytes) / crate::numeric_cast::u64_to_f64(KB)
        )
    } else {
        format!("{bytes} B")
    }
}

#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub struct GlobalProgressManager {
    multi: MultiProgress,
    main_bar: Option<ProgressBar>,
    sub_bar: Option<ProgressBar>,
}

impl GlobalProgressManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            multi: MultiProgress::new(),
            main_bar: None,
            sub_bar: None,
        }
    }

    /// Create the main progress bar.
    ///
    /// # Panics
    /// Panics if the template is invalid.
    pub fn create_main(&mut self, total: u64, prefix: &str) -> &ProgressBar {
        let bar = self.multi.add(ProgressBar::new(total));

        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_bar()
                    .template(progress_style::BATCH_TEMPLATE)
                    .expect("Invalid progress bar template")
                    .progress_chars(progress_style::PROGRESS_CHARS)
                    .tick_chars(progress_style::SPINNER_CHARS),
            );
            bar.set_prefix(prefix.to_string());
            bar.enable_steady_tick(Duration::from_millis(8));
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
        }
        self.main_bar.insert(bar)
    }

    /// Create a sub-spinner.
    ///
    /// # Panics
    /// Panics if the template is invalid.
    pub fn create_sub(&mut self, prefix: &str) -> &ProgressBar {
        let bar = self.multi.add(ProgressBar::new_spinner());

        if crate::progress_mode::is_quiet_mode() {
            bar.set_draw_target(ProgressDrawTarget::hidden());
        } else {
            bar.set_style(
                ProgressStyle::default_spinner()
                    .template(SUB_SPINNER_TEMPLATE)
                    .expect("Invalid spinner template")
                    .tick_chars(progress_style::SPINNER_CHARS),
            );
            bar.set_prefix(prefix.to_string());
            bar.enable_steady_tick(Duration::from_millis(8));
            bar.set_draw_target(ProgressDrawTarget::stderr_with_hz(120));
        }
        self.sub_bar.insert(bar)
    }

    pub fn inc_main(&self) {
        if let Some(bar) = &self.main_bar {
            bar.inc(1);
        }
    }

    pub fn set_main_message(&self, msg: &str) {
        if let Some(bar) = &self.main_bar {
            bar.set_message(msg.to_string());
        }
    }

    pub fn set_sub_message(&self, msg: &str) {
        if let Some(bar) = &self.sub_bar {
            bar.set_message(msg.to_string());
        }
    }

    pub fn finish_all(&self, summary: &str) {
        if let Some(bar) = &self.sub_bar {
            bar.finish_and_clear();
        }
        if let Some(bar) = &self.main_bar {
            bar.finish_with_message(summary.to_string());
        }
    }
}

impl Default for GlobalProgressManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use console::measure_text_width;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(1_048_576), "1.00 MB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m 1s");
    }

    #[test]
    fn test_truncate_filename() {
        assert_eq!(truncate_filename("short.txt", 20), "short.txt");
        let truncated = truncate_filename("very_long_filename_that_needs_truncation.txt", 20);
        assert!(measure_text_width(&truncated) <= 20);
        assert!(truncated.contains('…'));
    }

    #[test]
    fn test_build_coarse_progress_line_fits_narrow_terminal() {
        let stats = crate::progress_mode::get_current_stats_string();
        let line = build_coarse_progress_line(
            "Running",
            42.5,
            17,
            40,
            Duration::from_secs(95),
            "2m10s",
            "a_very_long_filename_that_should_be_trimmed_for_narrow_windows.jpeg",
            &stats,
            72,
        );
        assert!(
            measure_text_width(&line) <= 71,
            "line width {} exceeds terminal budget",
            measure_text_width(&line)
        );
    }
}

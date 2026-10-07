//! Elapsed time spinner with terminal title updates.
//! Mirrors `_fmt_elapsed()` and `spinner_run()` from
//! drag_and_drop_processor.py.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Format elapsed seconds to human-readable string.
pub fn format_elapsed(elapsed: Duration) -> String {
    let t = elapsed.as_secs();
    let s = t % 60;
    let m = (t / 60) % 60;
    let h = (t / 3600) % 24;
    let d = (t / 86400) % 7;
    let w = (t / (7 * 86400)) % 4;
    let mo = (t / (30 * 86400)) % 12;
    let y = t / (365 * 86400);

    if y > 0 {
        format!(
            "{:02}Y   {:02}M   {:02}W   {:02}D   {:02}h  {:02}m{:02}s",
            y, mo, w, d, h, m, s
        )
    } else if mo > 0 {
        format!(
            "{:02}M   {:02}W   {:02}D   {:02}h  {:02}m{:02}s",
            mo, w, d, h, m, s
        )
    } else if w > 0 {
        format!("{:02}W   {:02}D   {:02}h  {:02}m{:02}s", w, d, h, m, s)
    } else if d > 0 {
        format!("{:02}D   {:02}h  {:02}m{:02}s", d, h, m, s)
    } else if h > 0 {
        format!("{:02}h  {:02}m{:02}s", h, m, s)
    } else if m > 0 {
        format!("{:02}m{:02}s", m, s)
    } else {
        format!("{:02}s", s)
    }
}

pub fn update_terminal_title_direct(_elapsed: Duration) {
    #[cfg(target_os = "macos")]
    {
        let formatted = format_elapsed(_elapsed);
        terminal_control(&format!("\x1B]0;{formatted}\x07"));
    }
}

/// Update terminal title with elapsed time (macOS terminal escape).
pub fn update_terminal_title(elapsed: Duration) {
    update_terminal_title_direct(elapsed);
}

/// Print final elapsed time.
pub fn print_elapsed(elapsed: Duration) {
    println!("   Total time: {}", format_elapsed(elapsed));
}

pub struct ElapsedSpinner {
    stop_signal: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ElapsedSpinner {
    pub fn start() -> Self {
        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_signal.clone();
        let handle = thread::spawn(move || {
            let start = Instant::now();
            while !stop_clone.load(Ordering::Relaxed) {
                update_terminal_title_direct(start.elapsed());
                thread::sleep(Duration::from_millis(150));
            }
        });
        Self {
            stop_signal,
            handle: Some(handle),
        }
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.stop_signal.store(true, Ordering::Relaxed);
            let _ = handle.join();
            #[cfg(target_os = "macos")]
            {
                terminal_control("\x1B]0;\x07");
            }
        }
    }
}

impl Drop for ElapsedSpinner {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn hide_cursor() {
    terminal_control("\x1B[?25l");
}

pub fn show_cursor() {
    terminal_control("\x1B[?25h");
}

pub fn clear_screen() {
    terminal_control("\x1B[2J\x1B[H");
}

pub fn resize_terminal(rows: u16, cols: u16) {
    terminal_control(&format!("\x1B[8;{rows};{cols}t"));
}

fn terminal_control(sequence: &str) {
    let mut stdout = std::io::stdout().lock();
    let is_terminal = stdout.is_terminal();
    write_terminal_control(&mut stdout, is_terminal, sequence);
}

fn write_terminal_control(output: &mut impl Write, is_terminal: bool, sequence: &str) {
    // GUI pipes and machine result lines must never contain terminal controls.
    if is_terminal {
        let _ = output.write_all(sequence.as_bytes());
        let _ = output.flush();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn terminal_controls_never_prefix_piped_results() {
        let sequence = "\x1b]0;01s\x07";
        let mut output = Vec::new();
        super::write_terminal_control(&mut output, false, sequence);
        assert!(output.is_empty());
        super::write_terminal_control(&mut output, true, sequence);
        assert_eq!(output, sequence.as_bytes());
    }
}

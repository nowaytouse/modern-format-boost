use foundation::infra::static_logs::log_task_start;
use foundation::{LogConfig, init_logging, log_debug, log_info};

fn main() {
    // 1. Initialize logging system using unified log directory
    let log_dir = foundation::logging::LogConfig::unified_log_dir().join("dev_verify");
    let log_dir_display = log_dir.display().to_string();
    let _ = std::fs::remove_dir_all(&log_dir);
    std::fs::create_dir_all(&log_dir).unwrap();

    // Set RUST_LOG to debug to see debug logs
    // Note: set_var is unsafe in recent Rust versions
    unsafe {
        std::env::set_var("RUST_LOG", "debug");
    }

    let config = LogConfig::new()
        .with_log_dir(log_dir)
        .with_max_file_size(1024 * 1024)
        .with_max_files(5);

    let _ = init_logging("verify_log", &config);

    // 2. Emit various log patterns
    log_task_start("Verification Task: Premium Alignment");

    log_info!(
        label = "Setup",
        "Validating the new vertical alignment system..."
    );
    log_info!(label = "Probe", "Testing 10-character right-aligned label.");
    log_info!(
        label = "Metadata",
        "This should be perfectly aligned with the line above."
    );

    log_debug!(
        label = "Debug",
        "This is a debug message, should be visible in verify log."
    );
    log_debug!(
        label = "Probe",
        "This is the 'bit_rate' noise that should be at DEBUG level."
    );

    log_info!(label = "Check", "Verifying ANSI stripping in .log file...");

    // Wait for a bit to ensure logs are flushed
    std::thread::sleep(core::time::Duration::from_millis(100));

    foundation::log_info!("Info", "Verification logs emitted to {}/", log_dir_display);
    foundation::log_info!(
        "Info",
        "Please check the generated .log file for alignment and zero ANSI codes."
    );
}

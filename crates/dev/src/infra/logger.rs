//! Global logging setup wrapper using `foundation`.

use anyhow::Result;
use foundation::logging::{self, LogConfig};

/// Setup rotating file logger + stderr logger matching python `mfb_logger` setup.
///
/// # Errors
/// Returns an error if logging initialization fails.
pub fn setup_logger(program_name: &str) -> Result<()> {
    let config = LogConfig {
        max_file_size: 10 * 1024 * 1024, // 10 MiB limit matching Python MaxBytes
        ..LogConfig::default()
    };
    logging::init(program_name, &config)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn test_setup_logger() {
        // Since logging::init can only be successfully initialized once per process,
        // it might return Ok(()) or Err(_) depending on whether other tests already ran it.
        // We just ensure calling it doesn't panic.
        let _ = setup_logger("test_program");
    }
}

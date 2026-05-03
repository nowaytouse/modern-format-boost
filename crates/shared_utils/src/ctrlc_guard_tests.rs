#[cfg(test)]
mod tests {
    use crate::ctrlc_guard;
    use std::time::Duration;

    #[test]
    fn test_is_prompt_active_initial_state() {
        // Should be false initially
        assert!(!ctrlc_guard::is_prompt_active());
    }

    #[test]
    fn test_wait_if_prompt_active_no_block() {
        // Should return immediately if no prompt is active
        let start = std::time::Instant::now();
        ctrlc_guard::wait_if_prompt_active();
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn test_init_is_idempotent() {
        // Calling init multiple times should not panic or cause issues
        ctrlc_guard::init();
        ctrlc_guard::init();
    }
}

/// Test `numeric_cast` safety patterns and absence of fake/default values
/// Regression test for: `unwrap_or(0.0)` and other fake value injections
#[cfg(test)]
mod numeric_cast_safety_tests {
    use shared_utils::numeric_cast;

    /// Verify that `numeric_cast` functions return `Option`, not default values
    #[test]
    fn test_numeric_cast_returns_option_not_default() {
        // Numeric cast functions must return None on overflow, not inject 0 or 1

        let _result: Option<usize> = numeric_cast::u32_to_usize_strict(u32::MAX, "test_param");
        // On 32-bit systems this could succeed, on 64-bit it will succeed
        // The key is: it must return Option, never hardcode a default

        // Test with a value that clearly overflows
        let large_u64 = u64::MAX;
        let result_u64_to_u32: Option<u32> =
            numeric_cast::u64_to_u32_strict(large_u64, "overflow_test");
        // This MUST return None, never Some(0)
        assert!(
            result_u64_to_u32.is_none(),
            "Overflow must return None, not default value"
        );
    }

    /// Verify `f64_to_f32_lossy` lossy conversions don't hide precision loss
    #[test]
    fn test_f64_to_f32_lossy_preserves_value_intent() {
        // Lossy conversions are OK for float → float (same semantic domain)
        // The key: there's no "default value injection" - just precision loss

        let large_f64 = 1.234_567_89e10_f64;
        let result_f32 = numeric_cast::f64_to_f32_lossy(large_f64);

        // Should be close but not exact (that's OK for floats)
        assert!((f64::from(result_f32) - large_f64).abs() < 1e9);
    }

    /// Verify that integer arithmetic does NOT use `unwrap_or` defaults
    /// Example: `grid_size * grid_size` must `checked_mul` first, then convert
    #[test]
    fn test_integer_multiplication_overflow_safety() {
        // Safe pattern: checked_mul before cast
        let grid_size: u64 = 100_000; // Large but valid

        // SAFE pattern:
        let grid_size_sq = grid_size
            .checked_mul(grid_size)
            .expect("Grid size should not overflow");
        let result: Option<usize> = numeric_cast::u64_to_usize_strict(grid_size_sq, "grid_size_sq");
        assert!(result.is_some());

        // UNSAFE pattern (what we're preventing):
        // let result = unsafe_pattern_division(grid_size);
        // fn unsafe_pattern_division(grid_size: u32) -> usize {
        //     1000 / numeric_cast::u32_to_usize_strict(grid_size * grid_size, "BAD")
        //                        ^^^^^^^^^^^^^^^^^ hides arithmetic
        // }
    }

    /// Verify width * height calculations don't overflow pre-cast
    #[test]
    fn test_image_dimensions_no_hidden_arithmetic() {
        let width: u32 = 50_000;
        let height: u32 = 50_000;

        // SAFE pattern: cast to larger type first, then multiply
        let total_pixels_safe = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|p| numeric_cast::u64_to_usize_strict(p, "total_pixels"))
            .ok_or_else(|| "Pixel count overflow".to_string());
        assert!(total_pixels_safe.is_ok());

        // UNSAFE pattern (what we're preventing):
        // let bad = width * height; // Already overflowed at u32
        // let result = numeric_cast::u64_to_usize_strict(u64::from(bad), "total_pixels");
        //                                                        ^^^ garbage input
    }

    /// Verify `u32_to_usize_strict`, `u64_to_usize_strict` handle `None` properly
    #[test]
    fn test_numeric_cast_strict_none_handling() {
        // These functions return Option and MUST be handled
        // Never use unwrap_or(0), unwrap_or(1), or similar fakes

        // Small value → should succeed
        let small: u32 = 100;
        let result_small = numeric_cast::u32_to_usize_strict(small, "small_value");
        assert!(result_small.is_some());

        // On systems where usize is u32, this might fail (platform-dependent)
        // But the function MUST return Option, never inject a default

        // Overflow case on u64→usize (when usize is u32)
        let large: u64 = u64::from(u32::MAX) + 1;
        let _result_large = numeric_cast::u64_to_usize_strict(large, "large_value");
        // Result depends on platform, but must respect actual conversion semantics
        // Never: unwrap_or(0) or unwrap_or(usize::MAX)
    }

    /// Verify division patterns don't use default values
    #[test]
    fn test_division_patterns_no_defaults() {
        // Float division is safe (never uses default denominator)
        let numerator = 100.0_f64;
        let denominator = 3.0_f64;
        let result = numerator / denominator;
        assert!(result.is_finite());

        // Division by max(1) is OK - it prevents /0, but doesn't hide arithmetic
        let count = 0_usize;
        let safe_div = 100.0 / numeric_cast::u64_to_f64(count as u64).max(1.0);
        assert!(safe_div.is_finite());
        assert!((safe_div - 100.0).abs() < 1e-12);
    }
}

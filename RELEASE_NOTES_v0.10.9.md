# Release v0.10.9

This release focuses on dramatically improving the reliability of the ImageMagick fallback pipeline, ensuring that complex or problematic images are handled gracefully instead of failing. It also enhances logging for better diagnostics.

### Bug Fixes & Enhancements

*   **Robust ImageMagick Fallback Pipeline**:
    *   **Fixed a critical logic bug** where the ImageMagick fallback pipeline was *never* called for `img-hevc` and `img-av1` after a direct `cjxl` failure. This is now corrected, significantly increasing conversion success rates.
    *   **Enhanced Grayscale ICC Conflict Handling**: The retry logic for grayscale images with incompatible RGB ICC profiles is now more robust. It intelligently falls back to stripping the profile and attempting conversion at different bit depths (`16-bit` then `8-bit`), which resolves conversion failures for files like `IMG_6171.jpeg`.
    *   **Fixed Silent Failures**: Added detailed logging for every step of the multi-attempt fallback pipeline. Users can now clearly see `✅ Attempt 2 succeeded` or `❌ Attempt 3 failed` in the logs, providing full transparency into the recovery process.
    *   **Broader Error Detection**: The `is_decode_or_pixel_cjxl_error` detection has been expanded to catch more `cjxl` pixel data and decoding errors, routing more files to the robust fallback pipeline.
    *   **Fixed a logical flaw in `compress` mode** where a successful fallback conversion could be incorrectly deleted if it wasn't smaller than the original. This ensures that even if larger, a valid conversion is kept.

*   **Code Quality**:
    *   Resolved `unused variable` compiler warnings for a cleaner build.
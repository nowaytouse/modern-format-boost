//! Image Quality Metrics Module
//!
//! Provides precise PSNR and SSIM calculations between images.
//! Uses standard algorithms:
//! - PSNR: Peak Signal-to-Noise Ratio with parallel MSE calculation
//! - SSIM: Structural Similarity Index with 11x11 Gaussian window (Wang et al. 2004)

use crate::Rational;
use crate::types::ssim::Ssim;
use image::{DynamicImage, GenericImageView, GrayImage};
use rayon::prelude::*;

const K1: f64 = crate::constants::SSIM_K1;
const K2: f64 = crate::constants::SSIM_K2;
const L: f64 = crate::constants::MAX_8BIT_VALUE_F64;
/// Wang et al. SSIM stability constants: (`k_i` * L)^2 to avoid division-by-zero in low-contrast regions.
const C1: f64 = (K1 * L) * (K1 * L);
const C2: f64 = (K2 * L) * (K2 * L);

const WINDOW_SIZE: usize = crate::constants::SSIM_WINDOW_SIZE;

fn get_gaussian_window() -> [[f64; WINDOW_SIZE]; WINDOW_SIZE] {
    let sigma = crate::constants::SSIM_GAUSSIAN_SIGMA;
    let mut window = [[0.0f64; WINDOW_SIZE]; WINDOW_SIZE];
    let center = crate::numeric_cast::usize_to_f64(WINDOW_SIZE / 2);
    let mut sum = 0.0_f64;

    for (i, row) in window.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            let x = crate::numeric_cast::usize_to_f64(i) - center;
            let y = crate::numeric_cast::usize_to_f64(j) - center;
            let g = (-((x * x + y * y) / (2.0 * sigma * sigma))).exp();
            *cell = g;
            sum += g;
        }
    }
    for row in &mut window {
        for cell in row.iter_mut() {
            *cell /= sum;
        }
    }
    window
}

#[must_use]
/// # Panics
///
/// Panics if the MSE calculation encounters an invalid state (NaN/Inf) during rounding.
pub fn calculate_psnr(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let (w1, h1) = original.dimensions();
    let (w2, h2) = converted.dimensions();

    if w1 != w2 || h1 != h2 {
        return None;
    }

    let orig_rgb = original.to_rgb8();
    let conv_rgb = converted.to_rgb8();

    let orig_pixels: Vec<_> = orig_rgb.pixels().collect();
    let conv_pixels: Vec<_> = conv_rgb.pixels().collect();

    let mse_sum: f64 = orig_pixels
        .par_iter()
        .zip(conv_pixels.par_iter())
        .map(|(p1, p2)| {
            let r_diff = f64::from(p1[0]) - f64::from(p2[0]);
            let g_diff = f64::from(p1[1]) - f64::from(p2[1]);
            let b_diff = f64::from(p1[2]) - f64::from(p2[2]);
            b_diff.mul_add(b_diff, r_diff.mul_add(r_diff, g_diff * g_diff))
        })
        .sum();

    // Calculate MSE with high precision when available
    let pixel_count = crate::numeric_cast::usize_to_f64(orig_pixels.len());
    let mse = if cfg!(feature = "high-precision") && !cfg!(feature = "ci-static-build") {
        #[cfg(feature = "high-precision")]
        {
            use rug::Integer;
            // mse_sum is a sum of squared differences (f64).
            // Convert to Integer only after summing to avoid heap allocs in loop.
            let mse_sum_int = Integer::from(crate::numeric_cast::f64_to_u64_sat(mse_sum.round()));
            let pixel_count_int = Integer::from(orig_pixels.len());
            let three_int = Integer::from(3);
            let denominator = Rational::from(three_int) * Rational::from(pixel_count_int);
            (Rational::from(mse_sum_int) / denominator).to_f64()
        }
        #[cfg(not(feature = "high-precision"))]
        {
            mse_sum / (3.0 * pixel_count)
        }
    } else {
        mse_sum / (3.0 * pixel_count)
    };

    if crate::numeric_cast::is_effectively_zero(
        mse,
        crate::numeric_cast::FloatContext::Accumulation,
    ) {
        return Some(f64::INFINITY);
    }

    let psnr = 10.0_f64 * (L * L / mse).log10();
    Some(psnr)
}

#[must_use]
/// # Panics
///
/// Panics if the coordinate mapping or pixel accumulation results in a non-finite rational value.
pub fn calculate_ssim(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let (w1, h1) = original.dimensions();
    let (w2, h2) = converted.dimensions();

    if w1 != w2 || h1 != h2 {
        return None;
    }

    let orig_gray = original.to_luma8();
    let conv_gray = converted.to_luma8();

    let width = crate::numeric_cast::u32_to_usize_strict(w1, "w1")?;
    let height = crate::numeric_cast::u32_to_usize_strict(h1, "h1")?;

    if width < WINDOW_SIZE || height < WINDOW_SIZE {
        return calculate_ssim_simple(original, converted);
    }

    let window = get_gaussian_window();

    let valid_width = width - WINDOW_SIZE + 1;
    let valid_height = height - WINDOW_SIZE + 1;

    let positions: Vec<(usize, usize)> = (0..valid_height)
        .flat_map(|y| (0..valid_width).map(move |x| (x, y)))
        .collect();

    let ssim_sum: f64 = positions
        .par_iter()
        .map(|&(x, y)| calculate_window_ssim(&orig_gray, &conv_gray, x, y, &window))
        .sum();

    if positions.is_empty() {
        return None;
    }
    let count = crate::numeric_cast::usize_to_f64(positions.len());
    let count_r = Rational::from_f64(count).expect("positions.len() is always positive and finite");
    let ssim_sum_r = Rational::from_f64(ssim_sum)?;
    Some((ssim_sum_r / count_r).to_f64())
}

fn calculate_window_ssim(
    orig: &GrayImage,
    conv: &GrayImage,
    x: usize,
    y: usize,
    window: &[[f64; WINDOW_SIZE]; WINDOW_SIZE],
) -> f64 {
    // Single read of the window to avoid repeated get_pixel (cache-friendly).
    let mut buf_x = [[0.0f64; WINDOW_SIZE]; WINDOW_SIZE];
    let mut buf_y = [[0.0f64; WINDOW_SIZE]; WINDOW_SIZE];
    for (i, row) in window.iter().enumerate() {
        for (j, _) in row.iter().enumerate() {
            let px = x + j;
            let py = y + i;
            // px and py are guaranteed to be within image bounds by the valid_width/valid_height calculation
            let pixel_x = crate::numeric_cast::usize_to_u32_strict(px, "px")
                .expect("px overflow in calculate_window_ssim");
            let pixel_y = crate::numeric_cast::usize_to_u32_strict(py, "py")
                .expect("py overflow in calculate_window_ssim");
            if let Some(r) = buf_x.get_mut(i)
                && let Some(c) = r.get_mut(j)
            {
                *c = f64::from(orig.get_pixel(pixel_x, pixel_y)[0]);
            }
            if let Some(r) = buf_y.get_mut(i)
                && let Some(c) = r.get_mut(j)
            {
                *c = f64::from(conv.get_pixel(pixel_x, pixel_y)[0]);
            }
        }
    }

    let mut mean_x = 0.0_f64;
    let mut mean_y = 0.0_f64;
    for (i, row) in window.iter().enumerate() {
        for (j, &w) in row.iter().enumerate() {
            // Bounds guaranteed: buf_x/buf_y are [WINDOW_SIZE][WINDOW_SIZE],
            // i and j iterate over the same window dimensions.
            mean_x = w.mul_add(buf_x[i][j], mean_x);
            mean_y = w.mul_add(buf_y[i][j], mean_y);
        }
    }

    let mut var_x = 0.0_f64;
    let mut var_y = 0.0_f64;
    let mut cov_xy = 0.0_f64;
    for (i, row) in window.iter().enumerate() {
        for (j, &w) in row.iter().enumerate() {
            let dx = buf_x[i][j] - mean_x;
            let dy = buf_y[i][j] - mean_y;
            var_x = (w * dx).mul_add(dx, var_x);
            var_y = (w * dy).mul_add(dy, var_y);
            cov_xy = (w * dx).mul_add(dy, cov_xy);
        }
    }

    let numerator = (2.0 * mean_x).mul_add(mean_y, C1) * 2.0f64.mul_add(cov_xy, C2);
    #[allow(
        clippy::suboptimal_flops,
        reason = "SSIM denominator: a literal `mean_x*mean_x + mean_y*mean_y` reads more clearly as the textbook SSIM formula than a chain of mul_add calls; not a hot path."
    )]
    let denominator = (mean_x * mean_x + mean_y * mean_y + C1) * (var_x + var_y + C2);

    numerator / denominator
}

#[cfg_attr(not(feature = "high-precision"), allow(clippy::clone_on_copy))]
fn calculate_ssim_simple(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let orig_gray = original.to_luma8();
    let conv_gray = converted.to_luma8();

    let n_u64 = u64::from(orig_gray.width()) * u64::from(orig_gray.height());
    if n_u64 < 2 {
        return None;
    }

    // Single-pass: compute sum_x, total_sum_y, sum_xx, sum_yy, products_sum_xy (no Vec allocation).
    let mut sum_x: u64 = 0;
    let mut total_sum_y: u64 = 0;
    let mut sum_xx: u64 = 0;
    let mut sum_yy: u64 = 0;
    let mut products_sum_xy: u64 = 0;

    for (p_orig, p_conv) in orig_gray.pixels().zip(conv_gray.pixels()) {
        let x = u64::from(p_orig[0]);
        let y = u64::from(p_conv[0]);
        sum_x += x;
        total_sum_y += y;
        sum_xx += x * x;
        sum_yy += y * y;
        products_sum_xy += x * y;
    }

    #[cfg(not(feature = "high-precision"))]
    let pixel_count_f64 = crate::numeric_cast::u64_to_f64(n_u64);
    #[cfg(not(feature = "high-precision"))]
    let n1_f64 = crate::numeric_cast::u64_to_f64(n_u64 - 1);

    #[cfg(feature = "high-precision")]
    {
        use rug::Integer;
        let n = Rational::from(Integer::from(n_u64));
        let x_total_rational = Rational::from(Integer::from(sum_x));
        let y_accumulated_rat = Rational::from(Integer::from(total_sum_y));
        let xx_sq_sum = Rational::from(Integer::from(sum_xx));
        let yy_sq_sum = Rational::from(Integer::from(sum_yy));
        let xy_cross_sum = Rational::from(Integer::from(products_sum_xy));

        let mean_x = x_total_rational / n.clone();
        let mean_y = y_accumulated_rat / n.clone();
        let n1 = n.clone() - Rational::from(1);

        let var_x = (xx_sq_sum - (n.clone() * mean_x.clone() * mean_x.clone())) / n1.clone();
        let var_y = (yy_sq_sum - (n.clone() * mean_y.clone() * mean_y.clone())) / n1.clone();
        let cov_xy = (xy_cross_sum - (n * mean_x.clone() * mean_y.clone())) / n1;

        let c1_rat = Rational::from_f64(C1).expect("C1 = (K1*L)^2 is always finite");
        let c2_rat = Rational::from_f64(C2).expect("C2 = (K2*L)^2 is always finite");

        let numerator = (Rational::from(2) * mean_x.clone() * mean_y.clone() + c1_rat.clone())
            * (Rational::from(2) * cov_xy + c2_rat.clone());
        let denominator =
            (mean_x.clone() * mean_x + mean_y.clone() * mean_y + c1_rat) * (var_x + var_y + c2_rat);

        if denominator == 0 {
            return Some(1.0);
        }
        Some((numerator / denominator).to_f64())
    }

    #[cfg(not(feature = "high-precision"))]
    {
        let mean_x = crate::numeric_cast::u64_to_f64(sum_x) / pixel_count_f64;
        let mean_y = crate::numeric_cast::u64_to_f64(total_sum_y) / pixel_count_f64;

        let x_sq_total_f64 = crate::numeric_cast::u64_to_f64(sum_xx);
        let y_sq_total_f64 = crate::numeric_cast::u64_to_f64(sum_yy);
        let xy_prod_total_f64 = crate::numeric_cast::u64_to_f64(products_sum_xy);

        let var_x = (pixel_count_f64 * mean_x).mul_add(-mean_x, x_sq_total_f64) / n1_f64;
        let var_y = (pixel_count_f64 * mean_y).mul_add(-mean_y, y_sq_total_f64) / n1_f64;
        let cov_xy = (pixel_count_f64 * mean_x).mul_add(-mean_y, xy_prod_total_f64) / n1_f64;

        let numerator = (2.0 * mean_x).mul_add(mean_y, C1) * 2.0f64.mul_add(cov_xy, C2);
        let denominator =
            (mean_x.mul_add(mean_x, mean_y.mul_add(mean_y, C1))) * (var_x + var_y + C2);

        if crate::numeric_cast::is_effectively_zero(
            denominator,
            crate::numeric_cast::FloatContext::Accumulation,
        ) {
            return Some(1.0);
        }
        Some(numerator / denominator)
    }
}

#[must_use]
/// # Panics
///
/// Panics if internal image resizing fails or if the SSIM calculation encounters an invalid numeric state.
pub fn calculate_ms_ssim(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let scales = 5;
    let weights = [
        0.044_8_f64,
        0.285_6_f64,
        0.300_1_f64,
        0.236_3_f64,
        0.133_3_f64,
    ];

    let mut orig = original.clone();
    let mut conv = converted.clone();
    let mut ms_ssim = 1.0_f64;
    let mut used_weight_sum = 0.0_f64;

    for (i, &weight) in weights.iter().enumerate().take(scales) {
        let (w, h) = orig.dimensions();
        // WINDOW_SIZE = 11, always fits u32; saturating cast is equivalent.
        let window_u32 = crate::numeric_cast::usize_to_u32_sat(WINDOW_SIZE);
        if w < window_u32 || h < window_u32 {
            break;
        }

        if let Some(ssim) = calculate_ssim(&orig, &conv) {
            used_weight_sum += weight;
            ms_ssim *= ssim.powf(weight);
        }

        if i < scales - 1 {
            orig = image::DynamicImage::ImageRgba8(image::imageops::resize(
                &orig.to_rgba8(),
                w / 2,
                h / 2,
                image::imageops::FilterType::Lanczos3,
            ));
            conv = image::DynamicImage::ImageRgba8(image::imageops::resize(
                &conv.to_rgba8(),
                w / 2,
                h / 2,
                image::imageops::FilterType::Lanczos3,
            ));
        }
    }

    // Normalize by actual weight sum so result stays in [0, 1] when only a subset of scales run.
    if used_weight_sum < 1e-10_f64 {
        return None;
    }
    Some(ms_ssim.powf(1.0 / used_weight_sum))
}

#[must_use]
pub fn psnr_quality_description(psnr: f64) -> &'static str {
    if psnr.is_infinite() {
        "Identical (lossless)"
    } else if psnr > 50.0 {
        "Excellent - virtually lossless"
    } else if psnr > 40.0 {
        "Very good - minimal visible difference"
    } else if psnr > 35.0 {
        "Good - acceptable quality"
    } else if psnr > 30.0 {
        "Fair - noticeable degradation"
    } else {
        "Poor - significant quality loss"
    }
}

#[must_use]
pub fn ssim_quality_description(ssim: f64) -> &'static str {
    Ssim::clamped(ssim).quality_description()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    #[test]
    fn test_ssim_quality_description() {
        assert_eq!(ssim_quality_description(1.0), "Identical");
        assert_eq!(ssim_quality_description(0.999), "Identical");
        assert_eq!(
            ssim_quality_description(0.98),
            "Excellent - virtually lossless"
        );
        assert_eq!(
            ssim_quality_description(0.93),
            "Very good - minimal visible difference"
        );
        assert_eq!(ssim_quality_description(0.89), "Good - acceptable quality");
        assert_eq!(
            ssim_quality_description(0.82),
            "Fair - noticeable degradation"
        );
        assert_eq!(
            ssim_quality_description(0.5),
            "Poor - significant quality loss"
        );
    }

    #[test]
    fn test_identical_images() {
        let img1 = DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |x, y| {
            image::Rgb([
                crate::numeric_cast::u32_to_u8_sat(x % 256),
                crate::numeric_cast::u32_to_u8_sat(y % 256),
                128,
            ])
        }));
        let img2 = img1.clone();

        let psnr = calculate_psnr(&img1, &img2);
        assert!(
            psnr.unwrap_or_else(|| panic!("missing metric value"))
                .is_infinite()
        );

        let ssim = calculate_ssim(&img1, &img2);
        assert!((ssim.unwrap_or_else(|| panic!("missing metric value")) - 1.0).abs() < 0.01_f64);
    }

    #[test]
    fn test_gaussian_window() {
        let window = get_gaussian_window();
        let sum: f64 = window.iter().flat_map(|row| row.iter()).sum();
        assert!((sum - 1.0).abs() < 1e-10_f64);
    }

    #[test]
    fn test_different_images() {
        let img1 = DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |_, _| {
            image::Rgb([255, 255, 255])
        }));
        let img2 =
            DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |_, _| image::Rgb([0, 0, 0])));

        let psnr = calculate_psnr(&img1, &img2);
        assert!(psnr.is_some());
        assert!(psnr.unwrap_or_else(|| panic!("missing metric value")) < 10.0_f64);

        let ssim = calculate_ssim(&img1, &img2);
        assert!(ssim.is_some());
        assert!(ssim.unwrap_or_else(|| panic!("missing metric value")) < 0.1_f64);
    }

    #[test]
    fn test_ssim_different_dimensions_returns_none() {
        let img1 = DynamicImage::ImageRgb8(RgbImage::from_fn(50, 50, |_, _| {
            image::Rgb([128, 128, 128])
        }));
        let img2 = DynamicImage::ImageRgb8(RgbImage::from_fn(60, 60, |_, _| {
            image::Rgb([128, 128, 128])
        }));
        assert!(calculate_ssim(&img1, &img2).is_none());
        assert!(calculate_psnr(&img1, &img2).is_none());
    }

    #[test]
    fn test_ssim_small_image_uses_simple_path() {
        // < 11x11 hits calculate_ssim_simple (unbiased variance path).
        let img1 =
            DynamicImage::ImageRgb8(RgbImage::from_fn(8, 8, |_, _| image::Rgb([100, 100, 100])));
        let img2 =
            DynamicImage::ImageRgb8(RgbImage::from_fn(8, 8, |_, _| image::Rgb([100, 100, 100])));
        let ssim = calculate_ssim(&img1, &img2);
        assert!(ssim.is_some());
        assert!(
            (ssim.unwrap_or_else(|| panic!("missing metric value")) - 1.0).abs() < 0.01_f64,
            "identical 8x8 should give SSIM ≈ 1, got {ssim:?}"
        );
    }

    #[test]
    fn test_ssim_constant_image_equals_one() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(20, 20, |_, _| {
            image::Rgb([255, 255, 255])
        }));
        let ssim = calculate_ssim(&img, &img);
        assert!(ssim.is_some());
        assert!((ssim.unwrap_or_else(|| panic!("missing metric value")) - 1.0).abs() < 1e-6_f64);
    }

    #[test]
    fn test_ms_ssim_identical() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([
                crate::numeric_cast::u32_to_u8_sat(x.wrapping_add(y) % 256),
                128,
                200,
            ])
        }));
        let result = calculate_ms_ssim(&img, &img);
        assert!(result.is_some());
        assert!(
            result.unwrap_or_else(|| panic!("missing metric value")) >= 0.99_f64
                && result.unwrap_or_else(|| panic!("missing metric value")) <= 1.01_f64
        );
    }

    #[test]
    fn test_ms_ssim_small_image_returns_none() {
        // No scale has size >= 11; used_weight_sum == 0 -> None.
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(10, 10, |_, _| image::Rgb([0, 0, 0])));
        let result = calculate_ms_ssim(&img, &img);
        assert!(result.is_none());
    }
}

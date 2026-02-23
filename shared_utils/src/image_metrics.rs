//! Image Quality Metrics Module
//!
//! Provides precise PSNR and SSIM calculations between images.
//! Uses standard algorithms:
//! - PSNR: Peak Signal-to-Noise Ratio with parallel MSE calculation
//! - SSIM: Structural Similarity Index with 11x11 Gaussian window (Wang et al. 2004)

use image::{DynamicImage, GenericImageView, GrayImage};
use rayon::prelude::*;

const K1: f64 = 0.01;
const K2: f64 = 0.03;
const L: f64 = 255.0;
/// Wang et al. SSIM stability constants: (k_i * L)^2 to avoid division-by-zero in low-contrast regions.
const C1: f64 = (K1 * L) * (K1 * L);
const C2: f64 = (K2 * L) * (K2 * L);

const WINDOW_SIZE: usize = 11;

fn get_gaussian_window() -> [[f64; WINDOW_SIZE]; WINDOW_SIZE] {
    let sigma = 1.5;
    let mut window = [[0.0f64; WINDOW_SIZE]; WINDOW_SIZE];
    let center = (WINDOW_SIZE / 2) as f64;
    let mut sum = 0.0;

    for (i, row) in window.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            let x = i as f64 - center;
            let y = j as f64 - center;
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
            let r_diff = p1[0] as f64 - p2[0] as f64;
            let g_diff = p1[1] as f64 - p2[1] as f64;
            let b_diff = p1[2] as f64 - p2[2] as f64;
            r_diff * r_diff + g_diff * g_diff + b_diff * b_diff
        })
        .sum();

    let pixel_count = orig_pixels.len() as f64;
    let mse = mse_sum / (3.0 * pixel_count);

    if mse < 1e-10 {
        return Some(f64::INFINITY);
    }

    let psnr = 10.0 * (L * L / mse).log10();
    Some(psnr)
}

pub fn calculate_ssim(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let (w1, h1) = original.dimensions();
    let (w2, h2) = converted.dimensions();

    if w1 != w2 || h1 != h2 {
        return None;
    }

    let orig_gray = original.to_luma8();
    let conv_gray = converted.to_luma8();

    let width = w1 as usize;
    let height = h1 as usize;

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
    let count = positions.len() as f64;
    Some(ssim_sum / count)
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
            buf_x[i][j] = orig.get_pixel(px as u32, py as u32)[0] as f64;
            buf_y[i][j] = conv.get_pixel(px as u32, py as u32)[0] as f64;
        }
    }

    let mut mean_x = 0.0;
    let mut mean_y = 0.0;
    for (i, row) in window.iter().enumerate() {
        for (j, &w) in row.iter().enumerate() {
            mean_x += w * buf_x[i][j];
            mean_y += w * buf_y[i][j];
        }
    }

    let mut var_x = 0.0;
    let mut var_y = 0.0;
    let mut cov_xy = 0.0;
    for (i, row) in window.iter().enumerate() {
        for (j, &w) in row.iter().enumerate() {
            let dx = buf_x[i][j] - mean_x;
            let dy = buf_y[i][j] - mean_y;
            var_x += w * dx * dx;
            var_y += w * dy * dy;
            cov_xy += w * dx * dy;
        }
    }

    let numerator = (2.0 * mean_x * mean_y + C1) * (2.0 * cov_xy + C2);
    let denominator = (mean_x * mean_x + mean_y * mean_y + C1) * (var_x + var_y + C2);

    numerator / denominator
}

fn calculate_ssim_simple(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let orig_gray = original.to_luma8();
    let conv_gray = converted.to_luma8();

    let n = (orig_gray.width() * orig_gray.height()) as f64;
    if n < 2.0 {
        return None;
    }

    // Single-pass: compute sum_x, sum_y, sum_xx, sum_yy, sum_xy (no Vec allocation).
    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut sum_xx = 0.0f64;
    let mut sum_yy = 0.0f64;
    let mut sum_xy = 0.0f64;
    for (p_orig, p_conv) in orig_gray.pixels().zip(conv_gray.pixels()) {
        let x = p_orig[0] as f64;
        let y = p_conv[0] as f64;
        sum_x += x;
        sum_y += y;
        sum_xx += x * x;
        sum_yy += y * y;
        sum_xy += x * y;
    }

    let mean_x = sum_x / n;
    let mean_y = sum_y / n;
    // Unbiased variance/covariance (Wang et al. sample estimator; consistent with windowed path).
    let n1 = n - 1.0;
    let var_x = (sum_xx - n * mean_x * mean_x) / n1;
    let var_y = (sum_yy - n * mean_y * mean_y) / n1;
    let cov_xy = (sum_xy - n * mean_x * mean_y) / n1;

    let numerator = (2.0 * mean_x * mean_y + C1) * (2.0 * cov_xy + C2);
    let denominator = (mean_x.powi(2) + mean_y.powi(2) + C1) * (var_x + var_y + C2);
    if denominator < 1e-10 {
        return Some(1.0);
    }
    Some(numerator / denominator)
}

pub fn calculate_ms_ssim(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let scales = 5;
    let weights = [0.0448, 0.2856, 0.3001, 0.2363, 0.1333];

    let mut orig = original.clone();
    let mut conv = converted.clone();
    let mut ms_ssim = 1.0;
    let mut used_weight_sum = 0.0;

    for i in 0..scales {
        let (w, h) = orig.dimensions();
        if w < WINDOW_SIZE as u32 || h < WINDOW_SIZE as u32 {
            break;
        }

        if let Some(ssim) = calculate_ssim(&orig, &conv) {
            used_weight_sum += weights[i];
            ms_ssim *= ssim.powf(weights[i]);
        }

        if i < scales - 1 {
            orig = orig.resize_exact(w / 2, h / 2, image::imageops::FilterType::Lanczos3);
            conv = conv.resize_exact(w / 2, h / 2, image::imageops::FilterType::Lanczos3);
        }
    }

    // Normalize by actual weight sum so result stays in [0, 1] when only a subset of scales run.
    if used_weight_sum < 1e-10 {
        return None;
    }
    Some(ms_ssim.powf(1.0 / used_weight_sum))
}

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

pub fn ssim_quality_description(ssim: f64) -> &'static str {
    if ssim >= 0.999 {
        "Identical"
    } else if ssim >= 0.98 {
        "Excellent - virtually lossless"
    } else if ssim >= 0.95 {
        "Very good - minimal visible difference"
    } else if ssim >= 0.90 {
        "Good - acceptable quality"
    } else if ssim >= 0.85 {
        "Fair - noticeable degradation"
    } else {
        "Poor - significant quality loss"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    #[test]
    fn test_identical_images() {
        let img1 = DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        }));
        let img2 = img1.clone();

        let psnr = calculate_psnr(&img1, &img2);
        assert!(psnr.unwrap().is_infinite());

        let ssim = calculate_ssim(&img1, &img2);
        assert!((ssim.unwrap() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_gaussian_window() {
        let window = get_gaussian_window();
        let sum: f64 = window.iter().flat_map(|row| row.iter()).sum();
        assert!((sum - 1.0).abs() < 1e-10);
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
        assert!(psnr.unwrap() < 10.0);

        let ssim = calculate_ssim(&img1, &img2);
        assert!(ssim.is_some());
        assert!(ssim.unwrap() < 0.1);
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
            (ssim.unwrap() - 1.0).abs() < 0.01,
            "identical 8x8 should give SSIM ≈ 1, got {:?}",
            ssim
        );
    }

    #[test]
    fn test_ssim_constant_image_equals_one() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(20, 20, |_, _| {
            image::Rgb([255, 255, 255])
        }));
        let ssim = calculate_ssim(&img, &img);
        assert!(ssim.is_some());
        assert!((ssim.unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_ms_ssim_identical() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([(x.wrapping_add(y) % 256) as u8, 128, 200])
        }));
        let result = calculate_ms_ssim(&img, &img);
        assert!(result.is_some());
        assert!(result.unwrap() >= 0.99 && result.unwrap() <= 1.01);
    }

    #[test]
    fn test_ms_ssim_small_image_returns_none() {
        // No scale has size >= 11; used_weight_sum == 0 -> None.
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(10, 10, |_, _| image::Rgb([0, 0, 0])));
        let result = calculate_ms_ssim(&img, &img);
        assert!(result.is_none());
    }
}

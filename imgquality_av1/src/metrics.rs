//! Image Quality Metrics Module
//! 
//! Provides precise PSNR and SSIM calculations between images.
//! Uses standard algorithms:
//! - PSNR: Peak Signal-to-Noise Ratio with parallel MSE calculation
//! - SSIM: Structural Similarity Index with 11x11 Gaussian window (Wang et al. 2004)

#![allow(clippy::needless_range_loop)]

use image::{DynamicImage, GenericImageView, GrayImage};
use rayon::prelude::*;

/// SSIM constants for 8-bit images (from Wang et al. 2004)
const K1: f64 = 0.01;
const K2: f64 = 0.03;
const L: f64 = 255.0;  // Dynamic range for 8-bit images
const C1: f64 = (K1 * L) * (K1 * L);  // 6.5025
const C2: f64 = (K2 * L) * (K2 * L);  // 58.5225

/// Window size for SSIM calculation (standard is 11x11)
const WINDOW_SIZE: usize = 11;

/// Gaussian weights for 11x11 window (sigma = 1.5)
fn get_gaussian_window() -> [[f64; WINDOW_SIZE]; WINDOW_SIZE] {
    let sigma = 1.5;
    let mut window = [[0.0f64; WINDOW_SIZE]; WINDOW_SIZE];
    let center = (WINDOW_SIZE / 2) as f64;
    let mut sum = 0.0;
    
    for i in 0..WINDOW_SIZE {
        for j in 0..WINDOW_SIZE {
            let x = i as f64 - center;
            let y = j as f64 - center;
            let g = (-((x * x + y * y) / (2.0 * sigma * sigma))).exp();
            window[i][j] = g;
            sum += g;
        }
    }
    
    // Normalize
    for i in 0..WINDOW_SIZE {
        for j in 0..WINDOW_SIZE {
            window[i][j] /= sum;
        }
    }
    
    window
}

/// Calculate PSNR (Peak Signal-to-Noise Ratio) between two images
/// Uses parallel processing for large images.
/// Returns PSNR in dB. Higher values indicate better quality.
/// PSNR > 40dB: Excellent, PSNR 30-40dB: Good, PSNR < 30dB: Poor
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
    
    // Parallel MSE calculation using rayon
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
        // Identical images
        return Some(f64::INFINITY);
    }
    
    let psnr = 10.0 * (L * L / mse).log10();
    Some(psnr)
}

/// Calculate SSIM (Structural Similarity Index) between two images
/// Uses 11x11 Gaussian window (standard algorithm from Wang et al. 2004)
/// Returns SSIM between 0.0 and 1.0. 1.0 means identical images.
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
    
    // For very small images, fall back to simple calculation
    if width < WINDOW_SIZE || height < WINDOW_SIZE {
        return calculate_ssim_simple(original, converted);
    }
    
    let window = get_gaussian_window();
    
    // Calculate SSIM for each window position in parallel
    let _half_win = WINDOW_SIZE / 2;
    let valid_width = width - WINDOW_SIZE + 1;
    let valid_height = height - WINDOW_SIZE + 1;
    
    let positions: Vec<(usize, usize)> = (0..valid_height)
        .flat_map(|y| (0..valid_width).map(move |x| (x, y)))
        .collect();
    
    let ssim_sum: f64 = positions
        .par_iter()
        .map(|&(x, y)| {
            calculate_window_ssim(&orig_gray, &conv_gray, x, y, &window)
        })
        .sum();
    
    let count = positions.len() as f64;
    Some(ssim_sum / count)
}

/// Calculate SSIM for a single window position
fn calculate_window_ssim(
    orig: &GrayImage,
    conv: &GrayImage,
    x: usize,
    y: usize,
    window: &[[f64; WINDOW_SIZE]; WINDOW_SIZE],
) -> f64 {
    let mut mean_x = 0.0;
    let mut mean_y = 0.0;
    let mut var_x = 0.0;
    let mut var_y = 0.0;
    let mut cov_xy = 0.0;
    
    // Calculate weighted means
    for i in 0..WINDOW_SIZE {
        for j in 0..WINDOW_SIZE {
            let px = x + j;
            let py = y + i;
            let w = window[i][j];
            let vx = orig.get_pixel(px as u32, py as u32)[0] as f64;
            let vy = conv.get_pixel(px as u32, py as u32)[0] as f64;
            mean_x += w * vx;
            mean_y += w * vy;
        }
    }
    
    // Calculate weighted variances and covariance
    for i in 0..WINDOW_SIZE {
        for j in 0..WINDOW_SIZE {
            let px = x + j;
            let py = y + i;
            let w = window[i][j];
            let vx = orig.get_pixel(px as u32, py as u32)[0] as f64;
            let vy = conv.get_pixel(px as u32, py as u32)[0] as f64;
            let dx = vx - mean_x;
            let dy = vy - mean_y;
            var_x += w * dx * dx;
            var_y += w * dy * dy;
            cov_xy += w * dx * dy;
        }
    }
    
    // SSIM formula
    let numerator = (2.0 * mean_x * mean_y + C1) * (2.0 * cov_xy + C2);
    let denominator = (mean_x * mean_x + mean_y * mean_y + C1) * (var_x + var_y + C2);
    
    numerator / denominator
}

/// Simple SSIM for small images (fallback)
fn calculate_ssim_simple(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let orig_gray = original.to_luma8();
    let conv_gray = converted.to_luma8();
    
    let pixel_count = (orig_gray.width() * orig_gray.height()) as f64;
    
    let orig_pixels: Vec<f64> = orig_gray.pixels().map(|p| p[0] as f64).collect();
    let conv_pixels: Vec<f64> = conv_gray.pixels().map(|p| p[0] as f64).collect();
    
    let mean_x: f64 = orig_pixels.iter().sum::<f64>() / pixel_count;
    let mean_y: f64 = conv_pixels.iter().sum::<f64>() / pixel_count;
    
    let var_x: f64 = orig_pixels.iter().map(|x| (x - mean_x).powi(2)).sum::<f64>() / pixel_count;
    let var_y: f64 = conv_pixels.iter().map(|y| (y - mean_y).powi(2)).sum::<f64>() / pixel_count;
    let cov_xy: f64 = orig_pixels.iter().zip(conv_pixels.iter())
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum::<f64>() / pixel_count;
    
    let numerator = (2.0 * mean_x * mean_y + C1) * (2.0 * cov_xy + C2);
    let denominator = (mean_x.powi(2) + mean_y.powi(2) + C1) * (var_x + var_y + C2);
    
    Some(numerator / denominator)
}

/// Calculate MS-SSIM (Multi-Scale SSIM) - more accurate for varying viewing distances
/// Returns MS-SSIM between 0.0 and 1.0
pub fn calculate_ms_ssim(original: &DynamicImage, converted: &DynamicImage) -> Option<f64> {
    let scales = 5;
    let weights = [0.0448, 0.2856, 0.3001, 0.2363, 0.1333];
    
    let mut orig = original.clone();
    let mut conv = converted.clone();
    let mut ms_ssim = 1.0;
    
    for i in 0..scales {
        let (w, h) = orig.dimensions();
        if w < WINDOW_SIZE as u32 || h < WINDOW_SIZE as u32 {
            break;
        }
        
        if let Some(ssim) = calculate_ssim(&orig, &conv) {
            ms_ssim *= ssim.powf(weights[i]);
        }
        
        // Downsample for next scale
        if i < scales - 1 {
            orig = orig.resize_exact(w / 2, h / 2, image::imageops::FilterType::Lanczos3);
            conv = conv.resize_exact(w / 2, h / 2, image::imageops::FilterType::Lanczos3);
        }
    }
    
    Some(ms_ssim)
}

/// Quality assessment description based on PSNR
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

/// Quality assessment description based on SSIM
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
        let img2 = DynamicImage::ImageRgb8(RgbImage::from_fn(100, 100, |_, _| {
            image::Rgb([0, 0, 0])
        }));
        
        let psnr = calculate_psnr(&img1, &img2);
        assert!(psnr.is_some());
        assert!(psnr.unwrap() < 10.0);  // Very different images
        
        let ssim = calculate_ssim(&img1, &img2);
        assert!(ssim.is_some());
        assert!(ssim.unwrap() < 0.1);  // Very different images
    }
}


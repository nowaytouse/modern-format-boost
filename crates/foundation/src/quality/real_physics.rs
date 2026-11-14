//! Advanced Computer Vision Feature Extraction Module
//!
//! Provides mathematically rigorous, scientifically valid feature extraction
//! for CBIR (Content-Based Image Retrieval). This module replaces naive
//! mathematical projections or simple pixel grids with robust,
//! translation-resistant signal processing metrics.
//!
//! # 225-Dimension Advanced Physical Features:
//! - [0..24] Color Moments (Mean, Var, Skew, Kurtosis across R,G,B,Y,Cb,Cr)
//! - [24..88] Frequency Domain (Top 64 DCT coefficients of Luminance)
//! - [88..124] Edge / HOG (Histogram of Oriented Gradients, 36 bins)
//! - [124..140] Spatial Grid Entropy (4x4 localized structural complexity)
//! - [140..156] Block Texture Variances (GLCM approximation)
//! - [156..215] Uniform Local Binary Patterns (LBP, 59 bins for texture
//!   analysis)
//! - [215..225] Luminance Decile Histogram (10 bins)
#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops
)]

use crate::numeric_cast;
use image::{DynamicImage, GrayImage, RgbImage};
use std::f32::consts::PI;
use std::sync::OnceLock;

static UNIFORM_LBP_MAP: OnceLock<[usize; 256]> = OnceLock::new();

pub(crate) const PHYSICS_225_DIMENSIONS: usize = 225;

fn get_uniform_lbp_map() -> &'static [usize; 256] {
    // 🛡️ Safe: This closure performs pure mathematical constant calculations and
    // will never panic. OnceLock ensures the table is built exactly once across
    // all threads.
    UNIFORM_LBP_MAP.get_or_init(|| {
        let mut map = [58usize; 256];
        let mut current_bin = 0;
        for i in 0..=255u8 {
            let mut transitions = 0;
            for j in 0..7 {
                if ((i >> j) & 1) != ((i >> (j + 1)) & 1) {
                    transitions += 1;
                }
            }
            if ((i >> 7) & 1) != (i & 1) {
                transitions += 1;
            }

            if transitions <= 2 {
                map[usize::from(i)] = current_bin;
                current_bin += 1;
            }
        }
        // 🛡️ MATHEMATICAL INVARIANT: P=8 circular uniform patterns must equal 58
        assert_eq!(
            current_bin, 58,
            "Uniform LBP mathematical invariant violation"
        );
        map
    })
}

/// Extracts exactly 225 dimensions of authentic computer vision physics
/// features.
///
/// # Panics
/// Panics if the internal feature assembly ever stops producing exactly
/// `PHYSICS_225_DIMENSIONS` values.
#[must_use]
pub fn extract_image_physics_225(img: &DynamicImage) -> Vec<f32> {
    let mut features = Vec::with_capacity(PHYSICS_225_DIMENSIONS);

    let rgb = img.to_rgb8();
    let luma = img.to_luma8();

    // 1. Color Moments (24 dims)
    features.extend(compute_color_moments(&rgb));

    // 2. DCT Frequency Domain (64 dims)
    features.extend(compute_dct(&luma));

    // 3. Gradient/Edge Histogram (36 dims)
    features.extend(compute_gradient_histogram(&luma));

    // 4. Spatial Grid Entropy (16 dims)
    features.extend(compute_grid_entropy(&luma));

    // 5. Texture / Block Variance (16 dims)
    features.extend(compute_texture_features(&luma));

    // 6. Uniform Local Binary Patterns (59 dims)
    features.extend(compute_uniform_lbp(&luma));

    // 7. Luminance Decile Histogram (10 dims)
    features.extend(compute_luminance_histogram(&luma));

    // 🛡️ DEFENSIVE ASSERTION: Ensure data integrity for other modules
    assert_eq!(
        features.len(),
        PHYSICS_225_DIMENSIONS,
        "Physics feature length must be exactly 225"
    );
    features
}

pub(crate) fn encode_normalized_physics_225(target: &mut [f32], offset: usize, physics: &[f32]) {
    for (index, &value) in physics.iter().enumerate().take(PHYSICS_225_DIMENSIONS) {
        if let Some(slot) = target.get_mut(offset + index) {
            *slot = normalize_physics_225_value(index, value);
        }
    }
}

pub(crate) fn normalize_physics_225_value(index: usize, value: f32) -> f32 {
    if !value.is_finite() {
        // 0.0 here is a substitution inside the 225-dim quality embedding, not a
        // measurement — surface it so model-input drift is traceable.
        tracing::debug!(
            target: "mfb.algorithm",
            index,
            "non-finite physics feature; encoding 0.0 in 225-dim embedding"
        );
        return 0.0;
    }

    let normalized = match index {
        // Color moments: mean, variance, skew, kurtosis across six channels.
        0..=23 => match index % 4 {
            0 => value,
            1 => value * 4.0,
            2 | 3 => (value + 10.0) / 20.0,
            _ => unreachable!(),
        },
        // DCT: preserve signed energy around zero instead of hard-clipping negatives.
        24..=87 => (value + 16.0) / 32.0,
        // HOG, grid entropy, LBP, luminance histogram are already normalized.
        88..=123 | 124..=139 | 156..=224 => value,
        // Texture variance is sqrt(var) over [0, 0.5].
        140..=155 => value * 2.0,
        _ => 0.0,
    };

    normalized.clamp(0.0, 1.0)
}

fn compute_color_moments(rgb: &RgbImage) -> Vec<f32> {
    // 6 channels: R, G, B, Y, Cb, Cr
    let capacity = numeric_cast::u64_to_usize_sat(u64::from(rgb.width()) * u64::from(rgb.height()));
    let mut channels: [Vec<f32>; 6] = std::array::from_fn(|_| Vec::with_capacity(capacity));
    for p in rgb.pixels() {
        let r = f32::from(p[0]) / 255.0;
        let g = f32::from(p[1]) / 255.0;
        let b = f32::from(p[2]) / 255.0;

        let y = 0.299 * r + 0.587 * g + 0.114 * b;
        let cb = 0.5 + (-0.168_736 * r - 0.331_264 * g + 0.5 * b);
        let cr = 0.5 + (0.5 * r - 0.418_688 * g - 0.081_312 * b);

        channels[0].push(r);
        channels[1].push(g);
        channels[2].push(b);
        channels[3].push(y);
        channels[4].push(cb);
        channels[5].push(cr);
    }

    let mut moments = Vec::with_capacity(24);
    for c in channels {
        let n = numeric_cast::usize_to_f32_lossy(c.len());
        if n <= 1.0 {
            moments.extend_from_slice(&[0.0; 4]);
            continue;
        }

        let mean = c.iter().sum::<f32>() / n;
        let mut var_sum = 0.0;
        let mut skew_sum = 0.0;
        let mut kurt_sum = 0.0;

        for &val in &c {
            let diff = val - mean;
            let diff2 = diff * diff;
            var_sum += diff2;
            skew_sum += diff2 * diff;
            kurt_sum += diff2 * diff2;
        }

        let var = var_sum / n;
        let std_dev = var.sqrt();

        let skew = if std_dev > 1e-5 {
            (skew_sum / n) / (std_dev.powi(3))
        } else {
            0.0
        };

        let kurt = if var > 1e-5 {
            kurt_sum / (n * var * var) - 3.0
        } else {
            0.0
        };

        moments.push(mean);
        moments.push(var);
        moments.push(skew.clamp(-10.0, 10.0));
        moments.push(kurt.clamp(-10.0, 10.0));
    }
    moments
}

fn compute_dct(luma: &GrayImage) -> Vec<f32> {
    let resized = image::imageops::resize(luma, 8, 8, image::imageops::FilterType::Lanczos3);
    let pixels: Vec<f32> = resized
        .pixels()
        .iter()
        .map(|p| f32::from(p[0]) / 255.0)
        .collect();

    let mut dct = vec![0.0_f32; 64];
    for u in 0..8 {
        for v in 0..8 {
            let mut sum = 0.0;
            for x in 0..8 {
                for y in 0..8 {
                    let p = pixels[y * 8 + x];
                    let cos_x = ((2.0 * numeric_cast::usize_to_f32_lossy(x) + 1.0)
                        * numeric_cast::usize_to_f32_lossy(u)
                        * PI
                        / 16.0)
                        .cos();
                    let cos_y = ((2.0 * numeric_cast::usize_to_f32_lossy(y) + 1.0)
                        * numeric_cast::usize_to_f32_lossy(v)
                        * PI
                        / 16.0)
                        .cos();
                    sum += p * cos_x * cos_y;
                }
            }
            let cu = if u == 0 { 1.0 / 2.0_f32.sqrt() } else { 1.0 };
            let cv = if v == 0 { 1.0 / 2.0_f32.sqrt() } else { 1.0 };
            dct[v * 8 + u] = 0.25 * cu * cv * sum;
        }
    }
    dct
}

fn compute_gradient_histogram(luma: &GrayImage) -> Vec<f32> {
    let mut hist = vec![0.0_f32; 36];
    let (w, h) = (luma.width(), luma.height());
    if w < 3 || h < 3 {
        return hist;
    }

    let mut total_mag = 0.0;
    for y in 1..(h - 1) {
        for x in 1..(w - 1) {
            // 🛡️ RESTORED SOBEL 3x3: Robust gradient detection with noise suppression
            let p00 = f32::from(luma.get_pixel(x - 1, y - 1)[0]);
            let p10 = f32::from(luma.get_pixel(x, y - 1)[0]);
            let p20 = f32::from(luma.get_pixel(x + 1, y - 1)[0]);
            let p01 = f32::from(luma.get_pixel(x - 1, y)[0]);
            let p21 = f32::from(luma.get_pixel(x + 1, y)[0]);
            let p02 = f32::from(luma.get_pixel(x - 1, y + 1)[0]);
            let p12 = f32::from(luma.get_pixel(x, y + 1)[0]);
            let p22 = f32::from(luma.get_pixel(x + 1, y + 1)[0]);

            let gx = p20 + 2.0 * p21 + p22 - (p00 + 2.0 * p01 + p02);
            let gy = p02 + 2.0 * p12 + p22 - (p00 + 2.0 * p10 + p20);

            let mag = gx.hypot(gy);
            let angle = (gy.atan2(gx).to_degrees() + 360.0) % 360.0;
            let Some(bin) = numeric_cast::f32_to_usize_strict(angle / 10.0, "hog_angle_bin")
                .map(|value| value.min(35))
            else {
                continue;
            };
            hist[bin] += mag;
            total_mag += mag;
        }
    }

    if total_mag > 0.0 {
        for v in &mut hist {
            *v /= total_mag;
        }
    }
    hist
}

fn compute_grid_entropy(luma: &GrayImage) -> Vec<f32> {
    let mut entropies = vec![0.0_f32; 16];
    let (w, h) = (luma.width(), luma.height());
    let cell_w = w / 4;
    let cell_h = h / 4;
    if cell_w == 0 || cell_h == 0 {
        return entropies;
    }

    for row in 0..4 {
        for col in 0..4 {
            let mut hist = [0u32; 256];
            let mut count = 0;
            for y in (row * cell_h)..((row + 1) * cell_h) {
                for x in (col * cell_w)..((col + 1) * cell_w) {
                    hist[usize::from(luma.get_pixel(x, y)[0])] += 1;
                    count += 1;
                }
            }

            let mut entropy = 0.0_f32;
            if count > 0 {
                let count_f = numeric_cast::usize_to_f32_lossy(count);
                for &freq in &hist {
                    if freq > 0 {
                        let p = numeric_cast::u32_to_f32(freq) / count_f;
                        entropy -= p * p.log2();
                    }
                }
            }
            let Some(feature_index) =
                numeric_cast::u32_to_usize_strict(row * 4 + col, "grid_entropy_index")
            else {
                continue;
            };
            entropies[feature_index] = (entropy / 8.0).clamp(0.0, 1.0);
        }
    }
    entropies
}

fn compute_texture_features(luma: &GrayImage) -> Vec<f32> {
    let mut features = vec![0.0_f32; 16];
    let (w, h) = (luma.width(), luma.height());
    let (cell_w, cell_h) = (w / 4, h / 4);
    if cell_w == 0 || cell_h == 0 {
        return features;
    }

    for row in 0..4 {
        for col in 0..4 {
            let capacity = numeric_cast::u64_to_usize_sat(u64::from(cell_w) * u64::from(cell_h));
            let mut values = Vec::with_capacity(capacity);
            for y in (row * cell_h)..((row + 1) * cell_h) {
                for x in (col * cell_w)..((col + 1) * cell_w) {
                    values.push(f32::from(luma.get_pixel(x, y)[0]) / 255.0);
                }
            }
            // 🛡️ Defensive guard: ensure cell is not empty to prevent division by zero
            // (NaN)
            if values.is_empty() {
                continue;
            }
            let len_f = numeric_cast::usize_to_f32_lossy(values.len());
            let mean = values.iter().sum::<f32>() / len_f;
            let var = values.iter().map(|&v| (v - mean).powi(2)).sum::<f32>() / len_f;
            let Some(feature_index) =
                numeric_cast::u32_to_usize_strict(row * 4 + col, "texture_feature_index")
            else {
                continue;
            };
            features[feature_index] = var.sqrt();
        }
    }
    features
}

fn compute_uniform_lbp(luma: &GrayImage) -> Vec<f32> {
    let mut hist = vec![0.0_f32; 59];
    let (w, h) = (luma.width(), luma.height());
    if w < 3 || h < 3 {
        return hist;
    }

    let uniform_map = get_uniform_lbp_map();
    let mut total = 0.0;
    for y in 1..(h - 1) {
        for x in 1..(w - 1) {
            let center = luma.get_pixel(x, y)[0];
            let mut pattern = 0u8;
            let neighbors = [
                luma.get_pixel(x - 1, y - 1)[0],
                luma.get_pixel(x, y - 1)[0],
                luma.get_pixel(x + 1, y - 1)[0],
                luma.get_pixel(x + 1, y)[0],
                luma.get_pixel(x + 1, y + 1)[0],
                luma.get_pixel(x, y + 1)[0],
                luma.get_pixel(x - 1, y + 1)[0],
                luma.get_pixel(x - 1, y)[0],
            ];
            for (i, &n) in neighbors.iter().enumerate() {
                if n >= center {
                    pattern |= 1 << i;
                }
            }
            hist[uniform_map[usize::from(pattern)]] += 1.0;
            total += 1.0;
        }
    }
    if total > 0.0 {
        for v in &mut hist {
            *v /= total;
        }
    }
    hist
}

fn compute_luminance_histogram(luma: &GrayImage) -> Vec<f32> {
    let mut hist = vec![0.0_f32; 10];
    let mut total = 0.0;
    for p in luma.pixels() {
        let raw_bin = ((f32::from(p[0]) / 256.0) * 10.0).floor();
        let Some(bin) = numeric_cast::f32_to_usize_strict(raw_bin, "luminance_histogram_bin")
        else {
            continue;
        };
        hist[bin.clamp(0, 9)] += 1.0;
        total += 1.0;
    }
    if total > 0.0 {
        for v in &mut hist {
            *v /= total;
        }
    }
    hist
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn checkerboard_image(size: u32, cell: u32) -> DynamicImage {
        let mut raw = RgbImage::new(size, size);
        for x in 0..size {
            for y in 0..size {
                let val = if ((x / cell) + (y / cell)).is_multiple_of(2) {
                    255
                } else {
                    0
                };
                raw.put_pixel(x, y, image::Rgb([val, val, val]));
            }
        }
        DynamicImage::ImageRgb8(raw)
    }

    #[test]
    fn test_physics_dimension_count() {
        let img = DynamicImage::ImageRgb8(RgbImage::new(100, 100));
        let features = extract_image_physics_225(&img);

        // 1. Dimension Check
        assert_eq!(
            features.len(),
            225,
            "Physics features must be exactly 225 dimensions"
        );

        // 2. Baseline Finiteness Check
        assert!(
            features.iter().all(|&v| v.is_finite()),
            "Baseline features contain NaN or Inf"
        );
    }

    #[test]
    fn test_physics_signal_sensitivity() {
        // Different images should produce different features
        let img1 = DynamicImage::ImageRgb8(RgbImage::new(100, 100)); // All black
        let mut img2_raw = RgbImage::new(100, 100);
        for p in img2_raw.pixels_mut() {
            *p = image::Rgb([255, 128, 64]);
        }
        let img2 = DynamicImage::ImageRgb8(img2_raw); // Uniform color

        let f1 = extract_image_physics_225(&img1);
        let f2 = extract_image_physics_225(&img2);

        assert_ne!(f1, f2, "Features should differ for different pixel data");

        // Non-uniform image should have non-zero HOG/LBP and distributed bins
        let img3 = checkerboard_image(100, 10);
        let f3 = extract_image_physics_225(&img3);

        // 1. Finiteness Check (Catch real division/normalization issues on complex
        //    images)
        assert!(
            f3.iter().all(|&v| v.is_finite()),
            "Checkerboard features contain NaN or Inf"
        );

        // 2. HOG [88..124] should have signals
        let hog_sum: f32 = f3[88..124].iter().sum();
        assert!(hog_sum > 0.0, "Checkerboard image must produce HOG signal");

        // LBP [156..215] distribution check: should not be concentrated in a single bin
        let lbp_max: f32 = f3[156..215].iter().copied().fold(0.0_f32, f32::max);
        assert!(
            lbp_max < 1.0,
            "LBP should be distributed across multiple bins for non-uniform images"
        );
    }

    #[test]
    fn test_raw_physics_signal_exceeds_unit_interval_before_normalization() {
        let white =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 64, image::Rgb([255, 255, 255])));
        let checkerboard = checkerboard_image(64, 8);

        let white_features = extract_image_physics_225(&white);
        let checkerboard_features = extract_image_physics_225(&checkerboard);

        assert!(
            white_features.iter().any(|&value| value > 1.0),
            "Expected at least one raw physics feature above 1.0"
        );
        assert!(
            checkerboard_features.iter().any(|&value| value < 0.0),
            "Expected at least one raw physics feature below 0.0"
        );
    }

    #[test]
    fn test_normalized_physics_signal_is_finite_and_bounded() {
        let img = checkerboard_image(96, 12);
        let raw = extract_image_physics_225(&img);
        let mut normalized = vec![0.0_f32; PHYSICS_225_DIMENSIONS];
        encode_normalized_physics_225(&mut normalized, 0, &raw);

        assert_eq!(normalized.len(), PHYSICS_225_DIMENSIONS);
        assert!(normalized.iter().all(|value| value.is_finite()));
        assert!(normalized.iter().all(|value| (0.0..=1.0).contains(value)));
        assert_ne!(
            raw, normalized,
            "Normalization should transform signed/raw physics"
        );
    }

    #[test]
    fn test_normalize_physics_value_preserves_signed_structure() {
        assert!((normalize_physics_225_value(0, 0.25) - 0.25).abs() < f32::EPSILON);
        assert!((normalize_physics_225_value(1, 0.25) - 1.0).abs() < f32::EPSILON);
        assert!(normalize_physics_225_value(2, -10.0).abs() < f32::EPSILON);
        assert!((normalize_physics_225_value(3, 10.0) - 1.0).abs() < f32::EPSILON);
        assert!((normalize_physics_225_value(24, 0.0) - 0.5).abs() < 1.0e-6);
        assert!((normalize_physics_225_value(140, 0.5) - 1.0).abs() < f32::EPSILON);
        assert!((normalize_physics_225_value(156, 0.75) - 0.75).abs() < f32::EPSILON);
    }
}

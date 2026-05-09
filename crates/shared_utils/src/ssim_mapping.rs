//! PSNR→SSIM Dynamic Mapping Module
//!
//! v5.74: Used for transparency data prediction; does not affect search targets

use serde::{Deserialize, Serialize};

/// Uncalibrated PSNR→SSIM estimate used when no mapping/calibration is available.
/// Single formula shared by `explore_strategy` and other fallbacks so quality decisions are consistent.
#[inline]
#[must_use]
pub fn psnr_to_ssim_estimate(psnr_db: f64) -> f64 {
    if psnr_db.is_nan() || psnr_db <= 0.0_f64 {
        return 0.0;
    }
    // Heuristic: SSIM ≈ 1 - 10^(-PSNR/10) maps power-domain PSNR to a [0,1) quality
    // score. The /10 divisor (power domain) better separates high-quality encodes
    // (PSNR 35-50 dB) than the previous /20 (amplitude domain) which compressed
    // everything above 40 dB into the 0.99-0.9999 band, making it impossible to
    // distinguish quality levels during exploration fallback.
    //
    // At typical operating points:
    //   PSNR 25 dB → 0.997  (high quality, slightly overestimates vs real SSIM ~0.93)
    //   PSNR 30 dB → 0.999  (very high quality)
    //   PSNR 40 dB → 0.9999 (near-transparent)
    //
    // The overestimate at lower PSNR is acceptable because this is only used as a
    // fallback when actual SSIM measurement fails; the important property is monotonicity
    // and separation between quality levels.
    (1.0 - 10_f64.powf(-psnr_db / 10.0)).clamp(0.0, crate::constants::SSIM_MAPPING_CLAMP_MAX)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingPoint {
    pub psnr: f64,
    pub ssim: f64,
}

impl MappingPoint {
    #[inline]
    #[must_use]
    pub fn ssim_typed(&self) -> Option<crate::types::Ssim> {
        crate::types::Ssim::new(self.ssim).ok()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PsnrSsimMapping {
    points: Vec<MappingPoint>,
}

impl PsnrSsimMapping {
    #[must_use]
    pub const fn new() -> Self {
        Self { points: Vec::new() }
    }

    pub fn insert(&mut self, psnr: f64, ssim: f64) {
        if let Some(existing) = self.points.iter_mut().find(|point| {
            crate::numeric_cast::is_effectively_equal(
                point.psnr,
                psnr,
                crate::numeric_cast::FloatContext::ExactMatch,
            )
        }) {
            existing.ssim = ssim;
            return;
        }

        let point = MappingPoint { psnr, ssim };
        let pos = self
            .points
            .iter()
            .position(|p| p.psnr > psnr)
            .unwrap_or(self.points.len());
        self.points.insert(pos, point);
    }

    #[inline]
    fn interpolate_or_clamp(p1: &MappingPoint, p2: &MappingPoint, psnr: f64) -> f64 {
        let delta = p2.psnr - p1.psnr;
        if crate::numeric_cast::is_effectively_zero(
            delta,
            crate::numeric_cast::FloatContext::Accumulation,
        ) {
            return p2.ssim;
        }
        let ratio = (psnr - p1.psnr) / delta;
        ratio.mul_add(p2.ssim - p1.ssim, p1.ssim)
    }

    #[must_use]
    pub const fn has_enough_points(&self) -> bool {
        self.points.len() >= 3
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.points.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    #[must_use]
    pub fn predict_ssim_typed(&self, psnr: f64) -> Option<crate::types::Ssim> {
        self.predict_ssim(psnr)
            .and_then(|v| crate::types::Ssim::new(v).ok())
    }

    #[must_use]
    pub fn predict_ssim(&self, psnr: f64) -> Option<f64> {
        if self.points.len() < 2 {
            return None;
        }

        let mut lower = None;
        let mut upper = None;

        for (i, point) in self.points.iter().enumerate() {
            if point.psnr <= psnr {
                lower = Some(i);
            }
            if point.psnr >= psnr && upper.is_none() {
                upper = Some(i);
            }
        }

        match (lower, upper) {
            (Some(l), Some(u)) if l == u => self.points.get(l).map(|p| p.ssim),
            (Some(l), Some(u)) => {
                let p1 = self.points.get(l)?;
                let p2 = self.points.get(u)?;
                Some(Self::interpolate_or_clamp(p1, p2, psnr))
            }
            (Some(_), None) => {
                let n = self.points.len();
                if n >= 2 {
                    let p1 = self.points.get(n.saturating_sub(2))?;
                    let p2 = self.points.get(n.saturating_sub(1))?;
                    Some(Self::interpolate_or_clamp(p1, p2, psnr))
                } else {
                    None
                }
            }
            (None, Some(_)) if self.points.len() >= 2 => {
                let p1 = self.points.first()?;
                let p2 = self.points.get(1)?;
                Some(Self::interpolate_or_clamp(p1, p2, psnr))
            }
            _ => None,
        }
    }

    pub fn update(&mut self, psnr: f64, actual_ssim: f64) {
        const PSNR_TOLERANCE: f64 = crate::constants::SSIM_MAPPING_PSNR_TOLERANCE;
        if let Some(point) = self
            .points
            .iter_mut()
            .find(|p| (p.psnr - psnr).abs() < PSNR_TOLERANCE)
        {
            point.ssim = actual_ssim;
        } else {
            self.insert(psnr, actual_ssim);
        }
    }

    #[must_use]
    pub fn get_points(&self) -> &[MappingPoint] {
        &self.points
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_predict() {
        let mut mapping = PsnrSsimMapping::new();
        mapping.insert(30.0, 0.90);
        mapping.insert(40.0, 0.95);
        mapping.insert(50.0, 0.99);

        assert!(mapping.has_enough_points());

        assert!(
            (mapping
                .predict_ssim(40.0)
                .unwrap_or_else(|| panic!("missing predicted value"))
                - 0.95)
                .abs()
                < 0.001_f64
        );

        let predicted = mapping
            .predict_ssim(35.0)
            .unwrap_or_else(|| panic!("missing predicted value"));
        assert!((predicted - 0.925).abs() < 0.001_f64);
    }

    #[test]
    fn test_not_enough_points() {
        let mut mapping = PsnrSsimMapping::new();
        mapping.insert(30.0, 0.90);
        mapping.insert(40.0, 0.95);

        assert!(!mapping.has_enough_points());
        assert!(mapping.predict_ssim(35.0).is_some());
    }

    #[test]
    fn test_update() {
        let mut mapping = PsnrSsimMapping::new();
        mapping.insert(30.0, 0.90);
        mapping.update(30.2, 0.91);

        assert_eq!(mapping.len(), 1);
        assert!(
            (mapping
                .get_points()
                .first()
                .unwrap_or(&MappingPoint {
                    psnr: 0.0,
                    ssim: 0.0
                })
                .ssim
                - 0.91)
                .abs()
                < 0.001_f64
        );
    }

    #[test]
    fn test_insert_replaces_exact_duplicate_psnr() {
        let mut mapping = PsnrSsimMapping::new();
        mapping.insert(30.0, 0.90);
        mapping.insert(30.0, 0.92);

        assert_eq!(mapping.len(), 1);
        assert!(
            (mapping
                .get_points()
                .first()
                .unwrap_or(&MappingPoint {
                    psnr: 0.0,
                    ssim: 0.0
                })
                .ssim
                - 0.92)
                .abs()
                < 0.001_f64
        );
    }

    #[test]
    fn test_predict_ssim_with_duplicate_psnr_points_stays_finite() {
        let mapping = PsnrSsimMapping {
            points: vec![
                MappingPoint {
                    psnr: 30.0,
                    ssim: 0.90,
                },
                MappingPoint {
                    psnr: 30.0,
                    ssim: 0.92,
                },
            ],
        };

        let predicted = mapping
            .predict_ssim(35.0)
            .unwrap_or_else(|| panic!("prediction should exist"));
        assert!(predicted.is_finite());
        assert!((predicted - 0.92).abs() < 0.001_f64);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_linear_interpolation_correctness(
            p1_psnr in 20.0_f64..30.0_f64,
            p2_psnr in 35.0_f64..45.0_f64,
            p3_psnr in 50.0_f64..60.0_f64,
            p1_ssim in 0.85_f64..0.92_f64,
            p2_ssim in 0.93_f64..0.96_f64,
            p3_ssim in 0.97_f64..0.995_f64,
            query_ratio in 0.0_f64..1.0_f64,
        ) {
            let mut mapping = PsnrSsimMapping::new();
            mapping.insert(p1_psnr, p1_ssim);
            mapping.insert(p2_psnr, p2_ssim);
            mapping.insert(p3_psnr, p3_ssim);

            let query_psnr = query_ratio.mul_add(p2_psnr - p1_psnr, p1_psnr);
            let predicted = mapping.predict_ssim(query_psnr).unwrap_or_else(|| panic!("missing predicted value"));

            let expected = query_ratio.mul_add(p2_ssim - p1_ssim, p1_ssim);
            prop_assert!((predicted - expected).abs() < 0.000_1_f64,
                "Interpolation error: predicted={}, expected={}", predicted, expected);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_mapping_correction(
            psnr in 30.0_f64..50.0_f64,
            initial_ssim in 0.90_f64..0.95_f64,
            actual_ssim in 0.95_f64..0.99_f64,
        ) {
            let mut mapping = PsnrSsimMapping::new();
            mapping.insert(psnr, initial_ssim);

            mapping.update(psnr + 0.1, actual_ssim);

            let points = mapping.get_points();
            prop_assert_eq!(points.len(), 1, "Should update existing point");
            prop_assert!((points.first().unwrap_or(&MappingPoint { psnr: 0.0, ssim: 0.0 }).ssim - actual_ssim).abs() < 0.001_f64,
                "SSIM should be updated to actual value");
        }
    }
}

//! PSNR→SSIM 动态映射模块
//!
//! v5.74: 用于透明度数据预测，不影响搜索目标

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingPoint {
    pub psnr: f64,
    pub ssim: f64,
}

impl MappingPoint {

    #[inline]
    pub fn ssim_typed(&self) -> Option<crate::types::Ssim> {
        crate::types::Ssim::new(self.ssim).ok()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PsnrSsimMapping {
    points: Vec<MappingPoint>,
}

impl PsnrSsimMapping {
    pub fn new() -> Self {
        Self { points: Vec::new() }
    }

    pub fn insert(&mut self, psnr: f64, ssim: f64) {
        let point = MappingPoint { psnr, ssim };
        let pos = self
            .points
            .iter()
            .position(|p| p.psnr > psnr)
            .unwrap_or(self.points.len());
        self.points.insert(pos, point);
    }

    pub fn has_enough_points(&self) -> bool {
        self.points.len() >= 3
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn predict_ssim_typed(&self, psnr: f64) -> Option<crate::types::Ssim> {
        self.predict_ssim(psnr)
            .and_then(|v| crate::types::Ssim::new(v).ok())
    }

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
            (Some(l), Some(u)) if l == u => Some(self.points[l].ssim),
            (Some(l), Some(u)) => {
                let p1 = &self.points[l];
                let p2 = &self.points[u];
                let ratio = (psnr - p1.psnr) / (p2.psnr - p1.psnr);
                Some(p1.ssim + ratio * (p2.ssim - p1.ssim))
            }
            (Some(_), None) => {
                let n = self.points.len();
                if n >= 2 {
                    let p1 = &self.points[n - 2];
                    let p2 = &self.points[n - 1];
                    let ratio = (psnr - p1.psnr) / (p2.psnr - p1.psnr);
                    Some(p1.ssim + ratio * (p2.ssim - p1.ssim))
                } else {
                    None
                }
            }
            (None, Some(_)) => {
                if self.points.len() >= 2 {
                    let p1 = &self.points[0];
                    let p2 = &self.points[1];
                    let ratio = (psnr - p1.psnr) / (p2.psnr - p1.psnr);
                    Some(p1.ssim + ratio * (p2.ssim - p1.ssim))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn update(&mut self, psnr: f64, actual_ssim: f64) {
        const PSNR_TOLERANCE: f64 = 0.5;
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

        assert!((mapping.predict_ssim(40.0).unwrap() - 0.95).abs() < 0.001);

        let predicted = mapping.predict_ssim(35.0).unwrap();
        assert!((predicted - 0.925).abs() < 0.001);
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
        assert!((mapping.get_points()[0].ssim - 0.91).abs() < 0.001);
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
            p1_psnr in 20.0..30.0_f64,
            p2_psnr in 35.0..45.0_f64,
            p3_psnr in 50.0..60.0_f64,
            p1_ssim in 0.85..0.92_f64,
            p2_ssim in 0.93..0.96_f64,
            p3_ssim in 0.97..0.995_f64,
            query_ratio in 0.0..1.0_f64,
        ) {
            let mut mapping = PsnrSsimMapping::new();
            mapping.insert(p1_psnr, p1_ssim);
            mapping.insert(p2_psnr, p2_ssim);
            mapping.insert(p3_psnr, p3_ssim);

            let query_psnr = p1_psnr + query_ratio * (p2_psnr - p1_psnr);
            let predicted = mapping.predict_ssim(query_psnr).unwrap();

            let expected = p1_ssim + query_ratio * (p2_ssim - p1_ssim);
            prop_assert!((predicted - expected).abs() < 0.0001,
                "Interpolation error: predicted={}, expected={}", predicted, expected);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_mapping_correction(
            psnr in 30.0..50.0_f64,
            initial_ssim in 0.90..0.95_f64,
            actual_ssim in 0.95..0.99_f64,
        ) {
            let mut mapping = PsnrSsimMapping::new();
            mapping.insert(psnr, initial_ssim);

            mapping.update(psnr + 0.1, actual_ssim);

            let points = mapping.get_points();
            prop_assert_eq!(points.len(), 1, "Should update existing point");
            prop_assert!((points[0].ssim - actual_ssim).abs() < 0.001,
                "SSIM should be updated to actual value");
        }
    }
}

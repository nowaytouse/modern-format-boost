//! GPU calibration data structures

#[derive(Debug, Clone)]
pub struct CalibrationPoint {
    pub gpu_crf: f32,
    pub gpu_size: u64,
    pub gpu_ssim: Option<f64>,
    pub predicted_cpu_crf: f32,
    pub confidence: f64,
    pub reason: &'static str,
}

impl CalibrationPoint {
    #[must_use]
    pub fn from_gpu_result(
        gpu_crf: f32,
        gpu_size: u64,
        input_size: u64,
        gpu_ssim: Option<f64>,
        base_offset: f32,
    ) -> Self {
        let size_ratio =
            crate::numeric_cast::u64_to_f64(gpu_size) / crate::numeric_cast::u64_to_f64(input_size);

        let (adjustment, confidence, reason) = if size_ratio < 0.95_f64 {
            (
                1.0,
                0.85_f64,
                "GPU compression margin large, CPU can be more aggressive",
            )
        } else if size_ratio < 1.0_f64 {
            (
                0.5,
                0.90_f64,
                "GPU barely compressed, CPU slight adjustment",
            )
        } else if size_ratio < 1.05_f64 {
            (-0.5, 0.80_f64, "GPU slightly oversize, CPU needs lower CRF")
        } else {
            (-1.0, 0.70_f64, "GPU not compressed, CPU needs lower CRF")
        };

        let predicted_cpu_crf = (gpu_crf + base_offset + adjustment).clamp(10.0, 51.0);

        Self {
            gpu_crf,
            gpu_size,
            gpu_ssim,
            predicted_cpu_crf,
            confidence,
            reason,
        }
    }

    pub fn print_report(&self, input_size: u64) {
        if !crate::progress_mode::is_verbose_mode() {
            return;
        }
        let size_ratio = crate::numeric_cast::u64_to_f64(self.gpu_size)
            / crate::numeric_cast::u64_to_f64(input_size);
        let size_pct = (size_ratio - 1.0_f64) * 100.0_f64;

        eprintln!("┌─────────────────────────────────────────────────────");
        eprintln!("│ GPU→CPU Calibration Report");
        eprintln!("├─────────────────────────────────────────────────────");
        eprintln!(
            "│ GPU Boundary: CRF {:.1} → {:.1}% size",
            self.gpu_crf, size_pct
        );
        if let Some(ssim) = self.gpu_ssim {
            eprintln!("│ GPU SSIM: {ssim:.4}");
        }
        eprintln!("│ Predicted CPU Start: CRF {:.1}", self.predicted_cpu_crf);
        eprintln!("│ Confidence: {:.0}%", self.confidence * 100.0_f64);
        eprintln!("│ Reason: {}", self.reason);
        eprintln!("└─────────────────────────────────────────────────────");
    }
}

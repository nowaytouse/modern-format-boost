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
    pub fn from_gpu_result(
        gpu_crf: f32,
        gpu_size: u64,
        input_size: u64,
        gpu_ssim: Option<f64>,
        base_offset: f32,
    ) -> Self {
        let size_ratio = gpu_size as f64 / input_size as f64;

        let (adjustment, confidence, reason) = if size_ratio < 0.95 {
            (
                1.0,
                0.85,
                "GPU compression margin large, CPU can be more aggressive",
            )
        } else if size_ratio < 1.0 {
            (0.5, 0.90, "GPU barely compressed, CPU slight adjustment")
        } else if size_ratio < 1.05 {
            (-0.5, 0.80, "GPU slightly oversize, CPU needs lower CRF")
        } else {
            (-1.0, 0.70, "GPU not compressed, CPU needs lower CRF")
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
        let size_ratio = self.gpu_size as f64 / input_size as f64;
        let size_pct = (size_ratio - 1.0) * 100.0;

        eprintln!("┌─────────────────────────────────────────────────────");
        eprintln!("│ GPU→CPU Calibration Report");
        eprintln!("├─────────────────────────────────────────────────────");
        eprintln!(
            "│ GPU Boundary: CRF {:.1} → {:.1}% size",
            self.gpu_crf, size_pct
        );
        if let Some(ssim) = self.gpu_ssim {
            eprintln!("│ GPU SSIM: {:.4}", ssim);
        }
        eprintln!(
            "│ Predicted CPU Start: CRF {:.1}",
            self.predicted_cpu_crf
        );
        eprintln!("│ Confidence: {:.0}%", self.confidence * 100.0);
        eprintln!("│ Reason: {}", self.reason);
        eprintln!("└─────────────────────────────────────────────────────");
    }
}

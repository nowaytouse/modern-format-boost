use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ProcessHistory {
    pub software_version: String,
    pub analysis_timestamp: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Visual {
    pub average_luma: f64,
    pub peak_luma: f64,
    pub gray_center_of_mass: (f64, f64), // (x, y) normalized 0.0-1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_history_default() {
        let history = ProcessHistory::default();
        assert_eq!(history.software_version, "");
        assert_eq!(history.analysis_timestamp, None);
    }

    #[test]
    fn test_visual_default() {
        let visual = Visual::default();
        assert!(
            (visual.average_luma - 0.0).abs() < 1e-12,
            "average_luma not zero"
        );
        assert!((visual.peak_luma - 0.0).abs() < 1e-12, "peak_luma not zero");
        assert_eq!(visual.gray_center_of_mass, (0.0, 0.0));
    }

    #[test]
    fn test_visual_serde() {
        let visual = Visual {
            average_luma: 0.5,
            peak_luma: 0.9,
            gray_center_of_mass: (0.5, 0.5),
        };
        let json = serde_json::to_string(&visual).unwrap();
        assert!(json.contains("average_luma"));

        let deserialized: Visual = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, visual);
    }
}

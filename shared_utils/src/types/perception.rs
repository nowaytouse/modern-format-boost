use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ProcessHistory {
    pub software_version: String,
    pub analysis_timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct VisualPerception {
    pub average_luma: f64,
    pub peak_luma: f64,
    pub gray_center_of_mass: (f64, f64), // (x, y) normalized 0.0-1.0
}

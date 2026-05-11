#![allow(unused_imports)]

use shared_utils::{
    log_anomaly, log_corruption, log_detail, log_failure, log_fatal, log_hint, log_ignore,
    log_skip, log_success,
};

use std::fs;
fn main() {
    let data = fs::read("test.webp").unwrap();
    let has_anim = data.windows(4).any(|w| w == b"ANIM");
    log_detail!("Has ANIM: {has_anim}");
}

#![allow(unused_imports)]

use shared_utils::{
    log_anomaly, log_corruption, log_detail, log_failure, log_fatal, log_hint, log_ignore,
    log_skip, log_success,
};

fn main() {
    for _ in 0..1000 {
        let _f = tempfile::Builder::new().suffix(".log").tempfile().unwrap();
        // let file = f.reopen().unwrap();
    }
    log_detail!("Done");
}

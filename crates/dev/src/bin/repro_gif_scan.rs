#![allow(unused_imports)]

use shared_utils::{
    log_anomaly, log_corruption, log_detail, log_failure, log_fatal, log_hint, log_ignore,
    log_skip, log_success,
};

fn main() {
    let scan =
        shared_utils::media_meta_utils::scan_gif_headers(std::path::Path::new("/tmp/12-repro.gif"))
            .unwrap();
    log_detail!("{scan:?}");
}

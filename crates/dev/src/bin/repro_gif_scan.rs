#![allow(unused_imports)]

use foundation::{
    log_anomaly, log_corruption, log_debug, log_detail, log_failure, log_fatal, log_hint,
    log_ignore, log_info, log_skip, log_success,
};

fn main() {
    let scan =
        foundation::media_meta_utils::scan_gif_headers(std::path::Path::new("/tmp/12-repro.gif"))
            .unwrap();
    log_detail!("{scan:?}");
}

fn main() {
    let scan =
        shared_utils::media_meta_utils::scan_gif_headers(std::path::Path::new("/tmp/12-repro.gif"))
            .unwrap();
    println!("{scan:?}");
}

fn main() {
    let scan = modern_format_boost::media_meta_utils::scan_gif_headers(std::path::Path::new("/tmp/12-repro.gif")).unwrap();
    println!("{:?}", scan);
}

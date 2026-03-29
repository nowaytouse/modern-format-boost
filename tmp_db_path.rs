fn main() {
    let mut p = std::env::var("HOME").unwrap().to_string();
    p.push_str("/Library/Caches/modern_format_boost/gif_value_samples_v2.db");
    if std::path::Path::new(&p).exists() {
        println!("{}", p);
    }
}

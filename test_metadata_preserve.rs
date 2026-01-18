use std::path::Path;
use std::collections::HashMap;

fn main() {
    let src_dir = Path::new("/Users/nyamiiko/Downloads/all");
    let dst_dir = Path::new("/Users/nyamiiko/Downloads/all_optimized");
    
    let mut dir_metadata: HashMap<std::path::PathBuf, ()> = HashMap::new();
    
    // 模拟收集根目录
    dir_metadata.insert(src_dir.to_path_buf(), ());
    
    println!("Collected directories:");
    for (src_path, _) in dir_metadata.iter() {
        let rel_path = src_path.strip_prefix(src_dir).unwrap();
        let dst_path = dst_dir.join(rel_path);
        
        println!("  src: {:?}", src_path);
        println!("  rel: {:?}", rel_path);
        println!("  dst: {:?}", dst_path);
        println!("  dst == dst_dir: {}", dst_path == dst_dir);
        println!();
    }
}

fn main() {
    for _ in 0..1000 {
        let f = tempfile::Builder::new().suffix(".log").tempfile().unwrap();
        // let file = f.reopen().unwrap();
    }
    println!("Done");
}

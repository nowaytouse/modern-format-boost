fn u64_to_f64(val: u64) -> f64 {
    let high = u32::try_from(val >> 32).unwrap_or(0);
    let low = u32::try_from(val & 0xFFFFFFFF).unwrap_or(0);
    f64::from(high) * 4294967296.0 + f64::from(low)
}

fn main() {
    println!("{}", u64_to_f64(1234567890123456789));
}

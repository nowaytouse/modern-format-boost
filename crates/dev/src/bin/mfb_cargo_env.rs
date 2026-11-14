//! Optional PATH helper when `~/.cargo/bin` shims are broken.
//!
//! Root fix (run once): `repair_rustup_shims` bin.

use dev::infra::mfb_cargo_env::setup_cargo_env;

fn main() {
    let env = setup_cargo_env();
    let path = env.path_string.to_string_lossy();
    println!("export PATH=\"{path}\"");
    println!("export RUSTUP_TOOLCHAIN=\"{}\"", env.rustup_toolchain);
}

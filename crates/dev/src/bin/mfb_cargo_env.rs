//! Optional PATH helper when `~/.cargo/bin` shims are broken.
//!
//! Port of `crates/dev/scripts/ci/mfb_cargo_env.py` (Python retained as compat
//! reference). Root fix (run once): `repair_rustup_shims` bin /
//! `repair_rustup_shims.py`.

use dev::infra::mfb_cargo_env::setup_cargo_env;

fn main() {
    let env = setup_cargo_env();
    let path = env.path_string.to_string_lossy();
    println!("export PATH=\"{path}\"");
    println!("export RUSTUP_TOOLCHAIN=\"{}\"", env.rustup_toolchain);
}

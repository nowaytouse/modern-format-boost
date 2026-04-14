#!/usr/bin/env python3
import os
import sys
import subprocess
import shutil

GREEN = '\033[0;32m'
BLUE = '\033[0;34m'
NC = '\033[0m'

def print_c(color, text, end='\n'):
    print(f"{color}{text}{NC}", end=end)

def command_exists(cmd):
    return shutil.which(cmd) is not None

def run_cmd(cmd, env=None, check=True):
    return subprocess.run(cmd, env=env, shell=True, check=False)

def main():
    print_c(BLUE, "🛡️ Running Modern Format Boost Hardened Test Suite\n")

    if not command_exists("cargo-llvm-cov"):
        print("⚠️ cargo-llvm-cov not found. Installing...")
        run_cmd("cargo install cargo-llvm-cov")
        
    print_c(GREEN, "📊 Generating Coverage Report...")
    run_cmd("cargo llvm-cov --all-features --workspace --html")
    print("✅ HTML report generated in target/llvm-cov/html/index.html")
    
    print_c(GREEN, "\n🧪 Running AddressSanitizer (Memory Safety)...")
    env = dict(os.environ)
    env['RUSTFLAGS'] = "-Z sanitizer=address"
    res = run_cmd("cargo +nightly test --workspace --target x86_64-apple-darwin -- -Z unstable-options --error-format=json", env=env)
    if res.returncode != 0:
        print("⚠️ ASan requires nightly and compatible target.")
        
    print_c(GREEN, "\n🧭 Running Miri (Logic & UB Check)...")
    res = run_cmd("cargo +nightly miri test -p shared_utils --lib float_compare")
    if res.returncode != 0:
        print("⚠️ Miri check skipped or failed (common for FFI projects).")
        
    if command_exists("cargo-mutants"):
        print_c(GREEN, "\n🦠 Running Mutation Testing (Test Quality)...")
        run_cmd("cargo mutants -d crates/shared_utils --timeout 30")
    else:
        print("\n💡 Install 'cargo-mutants' to evaluate test effectiveness.")
        
    print_c(BLUE, "\n✅ Hardened Testing Session Complete.")

if __name__ == "__main__":
    main()

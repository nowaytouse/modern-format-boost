#!/usr/bin/env python3
# Build and install swift-corelibs-libdispatch for non-Apple CI hosts.

import os
import sys
import shutil
import subprocess
import multiprocessing

PREFIX = os.environ.get("LIBDISPATCH_PREFIX", "/usr/local")
SRC_DIR = os.environ.get("LIBDISPATCH_SRC_DIR", "/tmp/swift-corelibs-libdispatch")
REF = os.environ.get("LIBDISPATCH_REF", "main")

# Check if already installed
if os.path.exists(f"{PREFIX}/lib/libdispatch.so") or os.path.exists(
    f"{PREFIX}/lib/libdispatch.dylib"
):
    print(f"libdispatch already installed under {PREFIX}")
    sys.exit(0)

# Clone repo if not exists
if not os.path.exists(f"{SRC_DIR}/.git"):
    subprocess.check_call(
        [
            "git",
            "clone",
            "--depth",
            "1",
            "--branch",
            REF,
            "https://github.com/apple/swift-corelibs-libdispatch.git",
            SRC_DIR,
        ]
    )

# Compiler settings
CC = os.environ.get("CC", "clang")
CXX = os.environ.get("CXX", "clang++")

# CMake configuration
build_dir = f"{SRC_DIR}/build"
os.makedirs(build_dir, exist_ok=True)

subprocess.check_call(
    [
        "cmake",
        "-S",
        SRC_DIR,
        "-B",
        build_dir,
        "-DCMAKE_BUILD_TYPE=Release",
        f"-DCMAKE_INSTALL_PREFIX={PREFIX}",
        f"-DCMAKE_C_COMPILER={CC}",
        f"-DCMAKE_CXX_COMPILER={CXX}",
        "-DENABLE_SWIFT=OFF",
        "-DENABLE_TESTS=OFF",
    ]
)

# Parallel build
cpu_count = multiprocessing.cpu_count()
subprocess.check_call(["cmake", "--build", build_dir, "--parallel", str(cpu_count)])

# Sudo installation
subprocess.check_call(["sudo", "cmake", "--install", build_dir])

# ldconfig
if shutil.which("ldconfig"):
    try:
        subprocess.call(["sudo", "ldconfig"])
    except Exception:
        pass

# GITHUB_ENV updating
github_env = os.environ.get("GITHUB_ENV")
if github_env and os.path.exists(github_env):
    with open(github_env, "a") as f:
        pkg_config_path = os.environ.get("PKG_CONFIG_PATH", "")
        ld_library_path = os.environ.get("LD_LIBRARY_PATH", "")
        f.write(f"PKG_CONFIG_PATH={PREFIX}/lib/pkgconfig:{pkg_config_path}\n")
        f.write(f"LD_LIBRARY_PATH={PREFIX}/lib:{ld_library_path}\n")

#!/usr/bin/env bash
# Install media dependencies from their upstream projects for CI quality jobs.
set -euo pipefail

workspace="${GITHUB_WORKSPACE:-$(pwd)}"
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

download() {
  local url="$1"
  local output="$2"
  curl --fail --location --retry 3 --retry-all-errors --connect-timeout 30 \
    --output "$output" "$url"
}

prepend_media_paths() {
  local pkg_paths="/usr/local/lib/pkgconfig:/usr/local/lib/x86_64-linux-gnu/pkgconfig:/usr/lib/x86_64-linux-gnu/pkgconfig"
  local library_path="/usr/local/lib"
  export PKG_CONFIG_PATH="${pkg_paths}${PKG_CONFIG_PATH:+:${PKG_CONFIG_PATH}}"
  export LD_LIBRARY_PATH="${library_path}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
  if [[ -n "${GITHUB_ENV:-}" ]]; then
    {
      echo "PKG_CONFIG_PATH=${PKG_CONFIG_PATH}"
      echo "LD_LIBRARY_PATH=${LD_LIBRARY_PATH}"
    } >> "$GITHUB_ENV"
  fi
}

sudo apt-get update -qq
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev \
  librsvg2-dev libglib2.0-dev pkg-config clang cmake nasm ninja-build meson \
  libgmp-dev libmpfr-dev libmpc-dev libjxl-dev libjxl-tools libnuma-dev \
  libde265-dev libx264-dev libx265-dev libaom-dev libdav1d-dev libsvtav1-dev \
  exiftool imagemagick jpeginfo pngcheck exiv2 jhead libjpeg-turbo-progs \
  libavif-bin curl build-essential

cd "$workdir"

# Netflix VMAF is the authoritative provider of the libvmaf filter dependency.
git clone --depth 1 https://github.com/Netflix/vmaf.git vmaf
meson setup vmaf-build vmaf/libvmaf --prefix=/usr/local --buildtype=release \
  -Denable_docs=false -Denable_tests=false
ninja -C vmaf-build
sudo ninja -C vmaf-build install
sudo ldconfig
prepend_media_paths

# Use FFmpeg's own current development snapshot, not a third-party repackaging.
download "https://ffmpeg.org/releases/ffmpeg-snapshot.tar.bz2" ffmpeg-snapshot.tar.bz2
tar xjf ffmpeg-snapshot.tar.bz2
cd ffmpeg
./configure \
  --prefix=/usr/local \
  --enable-gpl \
  --enable-version3 \
  --enable-libx264 \
  --enable-libx265 \
  --enable-libaom \
  --enable-libdav1d \
  --enable-libsvtav1 \
  --enable-libvmaf \
  --disable-debug \
  --disable-doc
make -j"$(nproc)"
sudo make install
sudo ldconfig

ffmpeg -hide_banner -filters | grep -w libvmaf
ffmpeg -hide_banner -encoders | grep -w libx264
ffmpeg -hide_banner -encoders | grep -w libx265
ffmpeg -hide_banner -encoders | grep -w libaom-av1
ffmpeg -hide_banner -encoders | grep -w libsvtav1

# libheif 1.21 is required for the v1_21 API exercised by the quality suite.
cd "$workdir"
download "https://github.com/strukturag/libheif/releases/download/v1.21.0/libheif-1.21.0.tar.gz" libheif-src.tar.gz
tar xzf libheif-src.tar.gz
cmake -S libheif-1.21.0 -B libheif-build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr/local \
  -DCMAKE_WARN_DEPRECATED=OFF \
  -DWITH_EXAMPLES=OFF \
  -DWITH_TESTING=OFF
cmake --build libheif-build --parallel
sudo cmake --install libheif-build
sudo ldconfig

# gmp-mpfr-sys needs GNU MPC 1.4.1 when the system feature matrix is enabled.
# CI downloads this through the Rust mirror-aware helper first. Keep a local
# fallback for direct script use, but require a supplied archive to be valid.
cd "$workdir"
mpc_archive="${MFB_MPC_ARCHIVE:-}"
if [[ -z "$mpc_archive" ]]; then
  mpc_archive="$workdir/mpc.tar.xz"
  cargo run --locked --manifest-path "$workspace/Cargo.toml" -p dev --bin download_gnu_mpc -- "$mpc_archive"
elif [[ ! -s "$mpc_archive" ]]; then
  echo "MFB_MPC_ARCHIVE is missing or empty: $mpc_archive" >&2
  exit 1
fi
tar xf "$mpc_archive"
cd mpc-1.4.1
./configure --prefix=/usr/local --with-gmp=/usr --with-mpfr=/usr
make -j"$(nproc)"
sudo make install
sudo ldconfig
prepend_media_paths

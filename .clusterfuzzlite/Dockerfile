FROM gcr.io/oss-fuzz-base/base-builder-rust

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \n    m4 \
    cmake \
    nasm \
    ninja-build \
    meson \
    libheif-dev \
    libde265-dev \
    libx265-dev \
    libaom-dev \
    libdav1d-dev \
    libchromaprint-dev \
    libjpeg-dev \
    libpng-dev \

# Update Rust toolchain to latest nightly
RUN rustup toolchain install nightly && \
    rustup component add rust-src --toolchain nightly
ENV RUSTUP_TOOLCHAIN=nightly

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost
COPY .clusterfuzzlite/build.py $SRC/build.py
RUN chmod +x $SRC/build.py

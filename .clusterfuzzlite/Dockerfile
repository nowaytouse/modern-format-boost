FROM gcr.io/oss-fuzz-base/base-builder-rust

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    clang \
    g++ \
    m4 \
    cmake \
    nasm \
    ninja-build \
    meson \
    libde265-dev \
    libx265-dev \
    libaom-dev \
    libchromaprint-dev \
    libjpeg-dev \
    libpng-dev \
    libgmp-dev \
    libmpfr-dev \
    libmpc-dev

# Update Rust toolchain to latest nightly
RUN rustup toolchain install nightly && \
    rustup component add rust-src --toolchain nightly
ENV RUSTUP_TOOLCHAIN=nightly

# Set environment variables for gmp-mpfr-sys
ENV GMP_MPFR_SYS_CHECK=0
ENV GMP_DIR=/usr
ENV MPFR_DIR=/usr
ENV MPC_DIR=/usr
ENV GMP_LIB_DIR=/usr/lib/x86_64-linux-gnu
ENV GMP_INCLUDE_DIR=/usr/include
ENV MPFR_LIB_DIR=/usr/lib/x86_64-linux-gnu
ENV MPFR_INCLUDE_DIR=/usr/include
ENV MPC_LIB_DIR=/usr/lib/x86_64-linux-gnu
ENV MPC_INCLUDE_DIR=/usr/include
ENV ABI=64

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost
COPY .clusterfuzzlite/build.py $SRC/build.py
RUN printf '#!/bin/bash\npython3 $SRC/build.py\n' > /src/build.sh && \
    chmod +x /src/build.sh $SRC/build.py

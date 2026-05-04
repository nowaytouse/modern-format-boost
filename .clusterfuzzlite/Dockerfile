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
    libpng-dev

# Update Rust toolchain to latest nightly
RUN rustup toolchain install nightly && \
    rustup component add rust-src --toolchain nightly
ENV RUSTUP_TOOLCHAIN=nightly

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost
COPY .clusterfuzzlite/build.py $SRC/build.py
RUN printf '#!/bin/bash\npython3 $SRC/build.py\n' > /src/build.sh && \
    chmod +x /src/build.sh $SRC/build.py

FROM gcr.io/oss-fuzz-base/base-builder-rust

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    cmake \
    nasm \
    libheif-dev \
    libchromaprint-dev \
    libjpeg-dev \
    libpng-dev \
    libgmp-dev \
    libmpfr-dev \
    libmpc-dev

# Update Rust toolchain to latest nightly to support jpegxl-rs @ 1.92.0+
RUN rustup toolchain install nightly && \
    rustup default nightly && \
    rustup update


COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost
COPY .clusterfuzzlite/build.sh $SRC/build.sh
RUN chmod +x $SRC/build.sh

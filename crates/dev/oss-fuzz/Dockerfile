FROM gcr.io/oss-fuzz-base/base-fuzzer-rust

# Install system dependencies needed for shared_utils
RUN apt-get update && apt-get install -y \
    libheif-dev \
    libjpeg-dev \
    pkg-config

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost

COPY crates/dev/oss-fuzz/build.sh $SRC/build.sh

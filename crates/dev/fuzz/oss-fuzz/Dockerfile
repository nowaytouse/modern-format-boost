FROM gcr.io/oss-fuzz-base/base-builder-rust:latest@sha256:73be59e48262b7243f815f1c981a6e753e95e4b13e4d085cc9fc06bd79d42daa

# Install system dependencies needed for foundation
RUN apt-get update && apt-get install -y \
    libjpeg-dev libsharpyuv-dev libde265-dev libaom-dev libdav1d-dev \
    pkg-config cmake ninja-build curl sudo

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost

COPY crates/dev/fuzz/oss-fuzz/build.py $SRC/build.py
RUN chmod +x $SRC/build.py

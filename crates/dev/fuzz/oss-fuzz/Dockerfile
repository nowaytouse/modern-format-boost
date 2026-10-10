FROM gcr.io/oss-fuzz-base/base-builder-rust:latest@sha256:9178c8abf83e7a82d02c1a9bf7b2f564320f669f7a1cec512ae9a8c4ea078427

# Install system dependencies needed for foundation
RUN apt-get update && apt-get install -y \
    libjpeg-dev libsharpyuv-dev libde265-dev libaom-dev libdav1d-dev \
    pkg-config cmake ninja-build curl sudo

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost

COPY crates/dev/fuzz/oss-fuzz/build.py $SRC/build.py
RUN chmod +x $SRC/build.py

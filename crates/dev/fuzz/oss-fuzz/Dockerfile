FROM gcr.io/oss-fuzz-base/base-builder-rust:latest@sha256:a4756c3ece28bb41d612177302fa27a5bd132a378e9dcfd8ccb4289175eafda2

# Install system dependencies needed for foundation
RUN apt-get update && apt-get install -y \
    libjpeg-dev libsharpyuv-dev libde265-dev libaom-dev libdav1d-dev \
    pkg-config cmake ninja-build curl sudo

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost

COPY crates/dev/fuzz/oss-fuzz/build.py $SRC/build.py
RUN chmod +x $SRC/build.py

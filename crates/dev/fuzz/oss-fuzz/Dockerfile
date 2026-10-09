FROM gcr.io/oss-fuzz-base/base-builder-rust:latest@sha256:c62b4a41865adec0ffd93e9d21be7654a8359f75dee2e96a409325a494e93040

# Install system dependencies needed for foundation
RUN apt-get update && apt-get install -y \
    libjpeg-dev libsharpyuv-dev libde265-dev libaom-dev libdav1d-dev \
    pkg-config cmake ninja-build curl sudo

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost

COPY crates/dev/fuzz/oss-fuzz/build.py $SRC/build.py
RUN chmod +x $SRC/build.py

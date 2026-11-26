FROM gcr.io/oss-fuzz-base/base-builder-rust:latest@sha256:2216f0ac0a289a0e8974917779c3f744c354067dd81e0c8f2fc53bc1a9a573db

# Install system dependencies needed for foundation
RUN apt-get update && apt-get install -y \
    libjpeg-dev libsharpyuv-dev libde265-dev libaom-dev libdav1d-dev \
    pkg-config cmake ninja-build curl sudo

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost

COPY crates/dev/fuzz/oss-fuzz/build.py $SRC/build.py
RUN chmod +x $SRC/build.py

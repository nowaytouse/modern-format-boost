FROM gcr.io/oss-fuzz-base/base-builder-rust

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libheif-dev \
    libvmaf-dev \
    libchromaprint-dev \
    libjpeg-dev \
    libpng-dev

COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost
COPY .clusterfuzzlite/build.sh $SRC/build.sh
RUN chmod +x $SRC/build.sh

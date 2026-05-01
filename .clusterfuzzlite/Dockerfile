FROM gcr.io/oss-fuzz-base/base-builder-rust
COPY . $SRC/modern-format-boost
WORKDIR $SRC/modern-format-boost
COPY .clusterfuzzlite/build.sh $SRC/build.sh
chmod +x $SRC/build.sh

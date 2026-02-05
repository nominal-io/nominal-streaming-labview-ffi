#!/bin/bash
set -e

TARGET=${1:-armv7-unknown-linux-gnueabihf}

echo "Building for RT target: $TARGET"
cargo build --release --target=$TARGET

# Strip binary to reduce size
echo "Stripping binary..."
if [ "$TARGET" = "armv7-unknown-linux-gnueabihf" ]; then
    arm-linux-gnueabihf-strip target/$TARGET/release/libnominal_labview_ffi.so
elif [ "$TARGET" = "aarch64-unknown-linux-gnu" ]; then
    aarch64-linux-gnu-strip target/$TARGET/release/libnominal_labview_ffi.so
fi

echo "Done! Binary: target/$TARGET/release/libnominal_labview_ffi.so"
ls -lh target/$TARGET/release/libnominal_labview_ffi.so
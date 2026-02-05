#!/bin/bash
set -e

echo "Building for all platforms..."

# Desktop platforms
echo "Building for x86_64 Linux..."
cargo build --release --target=x86_64-unknown-linux-gnu

echo "Building for Windows..."
cargo build --release --target=x86_64-pc-windows-gnu

# RT platforms
echo "Building for ARMv7 (RT)..."
cargo build --release --target=armv7-unknown-linux-gnueabihf

echo "Building for ARM64 (RT)..."
cargo build --release --target=aarch64-unknown-linux-gnu

echo "Done! Binaries are in target/<platform>/release/"
default: ci

# Quick local check: lint, test
ci: clippy test

# Install the Rust targets the shared libraries are built for (matches CI)
setup:
    rustup target add x86_64-pc-windows-msvc i686-pc-windows-msvc x86_64-unknown-linux-gnu aarch64-apple-darwin

# Debug build of the shared library
build:
    cargo build

# Run tests
test:
    cargo test

# Run clippy lints
clippy:
    cargo clippy --all-targets

# Apply rust formatting
fmt:
    cargo fmt --all

# Check rust formatting
fmt-check:
    cargo fmt --all --check

# Release build for one target, as CI does; CI then renames it lib_nominal-streaming-lv_<bits>.<ext>
package target="x86_64-pc-windows-msvc":
    cargo build --release --target {{target}}

# Release build for a LabVIEW RT target, stripped (needs the target's cross linker)
package-rt target="armv7-unknown-linux-gnueabihf":
    bash scripts/build-rt.sh {{target}}

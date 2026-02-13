# justfile

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Default task
default: build bindings

# Build the ffi crate in release mode
build:
    cargo build -p ffi --release

# Generate all bindings (Kotlin & Swift)
bindings: build kotlin swift

# Generate Kotlin bindings
kotlin: build
    cargo run --bin generate-bindings generate \
      --library target/release/libffi.dylib \
      --language kotlin \
      --out-dir ffi/rust/uniffi-output

# Generate Swift bindings
swift: build
    cargo run --bin generate-bindings generate \
      --library target/release/libffi.dylib \
      --language swift \
      --out-dir ffi/rust/uniffi-output

# Clean build artifacts
clean:
    cargo clean

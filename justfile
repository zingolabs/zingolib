# justfile

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Runs `build` and `bindings`
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
      --out-dir ffi/rust/uniffi-output/kotlin

# Run all and
test-all-kotlin:
    ./ffi/android/ZingolibFfi/gradlew -p ./ffi/android/ZingolibFfi --continue \
      :zingolibffi:testDebugUnitTest \
      :zingolibffi:connectedDebugAndroidTest

cargo-ndk: kotlin
    cargo ndk \
        -t arm64-v8a \
        build --release -p ffi

copy-kt-bindings: kotlin
    cp -r ffi/rust/uniffi-output/kotlin/ZingolibFfi ffi/android/ZingolibFfi/zingolibffi/src/main/java/

# Generate Swift bindings
swift: build
    cargo run --bin generate-bindings generate \
      --library target/release/libffi.dylib \
      --language swift \
      --out-dir ffi/rust/uniffi-output/swift

# Clean build artifacts
clean:
    cargo clean

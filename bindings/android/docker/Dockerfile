# The tools of the Android build, with no sources and no environment. The workbench
# builder mounts this repository, runs the per-ABI steps in a container of
# this image, and sets their environment itself. build-n-push-android-image.yaml
# publishes the same image under the tag that binding-layer-android.yaml runs
# in, so CI installs nothing before it builds. zstd serves the job's cargo
# cache, which compresses with it when it is present.
FROM ghcr.io/zingolabs/android_builder:018

RUN apt update \
    && apt upgrade -y \
    && apt install -y --no-install-recommends --no-install-suggests \
    build-essential \
    cmake \
    golang \
    clang-18 \
    libclang-18-dev \
    gcc \
    g++ \
    zstd

# The builder passes rust-toolchain.toml verbatim and every Android target, so
# the image holds the toolchain that file selects, with its components and
# those targets, and the install step a run issues inside the mounted checkout
# finds nothing to add. The tools below build on that same toolchain, because
# the override file in this directory selects it.
ARG RUST_TOOLCHAIN_TOML
ARG RUST_TARGETS
WORKDIR /opt/toolchain
RUN test -n "$RUST_TOOLCHAIN_TOML" && test -n "$RUST_TARGETS" \
    || { echo 'build args RUST_TOOLCHAIN_TOML and RUST_TARGETS are required' >&2; exit 1; }
RUN printf '%s\n' "$RUST_TOOLCHAIN_TOML" > rust-toolchain.toml \
    && rustup toolchain install \
    && rustup target add $RUST_TARGETS

RUN cargo install --force --locked bindgen-cli

RUN cargo install --locked --version 4.0.1 cargo-ndk

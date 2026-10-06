# The tools of the Android build, with no sources and no environment. The workbench
# builder mounts this repository, runs the per-ABI steps in a container of
# this image, and sets their environment itself.
FROM docker.io/zingodevops/android_builder:018

RUN apt update \
    && apt upgrade -y \
    && apt install -y --no-install-recommends --no-install-suggests \
    build-essential \
    cmake \
    golang \
    clang-18 \
    libclang-18-dev \
    gcc \
    g++

# The builder passes zingolib's pinned channel and the Android targets, so a
# container starts with the toolchain that rust-toolchain.toml selects and
# installs nothing on each run.
ARG RUST_CHANNEL
ARG RUST_TARGETS
RUN test -n "$RUST_CHANNEL" && test -n "$RUST_TARGETS" \
    && rustup toolchain install "$RUST_CHANNEL" --profile minimal \
    && rustup target add --toolchain "$RUST_CHANNEL" $RUST_TARGETS \
    && rustup default "$RUST_CHANNEL"

RUN cargo install --force --locked bindgen-cli

RUN cargo install --version 4.0.1 cargo-ndk

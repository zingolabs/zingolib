# The tool layer of zingo-mobile's rust/android/docker/Dockerfile at the
# Freeze Commit, without its sources. The workbench builder mounts this
# repository and runs the per-ABI steps in a container of this image.
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

RUN rustup default stable

RUN cargo install --force --locked bindgen-cli

RUN cargo install --version 4.0.1 cargo-ndk

ENV LIBCLANG_PATH=/usr/lib/llvm-18/lib
ENV CARGO_NDK_PLATFORM=26
ENV CARGO_NDK_ANDROID_PLATFORM=26
ENV AR=llvm-ar
ENV LD=ld
ENV RANLIB=llvm-ranlib
ENV CFLAGS_aarch64_linux_android="-mno-outline-atomics"
ENV CXXFLAGS_aarch64_linux_android="-mno-outline-atomics"

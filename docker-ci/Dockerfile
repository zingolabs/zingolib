# No ARG defaults, deliberately: .env.testing-artifacts is the single
# source of truth for every pinned value (and rust-toolchain.toml for
# RUST_VERSION, which the Makefile derives). Build through
# `makers build-image`, which sources the env file and passes each
# value; a bare `docker build` without --build-args fails loudly
# instead of assembling an image from stale duplicated defaults.
ARG RUST_VERSION
ARG NEXTEST_VERSION

FROM debian:trixie-slim AS runner

ARG RUST_VERSION
ARG NEXTEST_VERSION

LABEL org.zingolib.test-artifacts.versions.rust="${RUST_VERSION}"
LABEL org.zingolib.test-artifacts.versions.nextest="${NEXTEST_VERSION}"

# Install dependencies and update ca certificates
RUN apt update \
    && apt install -y --no-install-recommends --no-install-suggests \
    python3 \
    git \
    gcc \
    curl \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    build-essential \
    libclang-dev \
    ca-certificates \
    gnupg \
    && update-ca-certificates

# Install pinned Rust tooling and test helpers
ENV HOME=/root
ENV CARGO_HOME=$HOME/.cargo
ENV RUSTUP_HOME=$HOME/.rustup
ENV PATH=$PATH:$CARGO_HOME/bin
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain "${RUST_VERSION}" --profile minimal \
    && rustup component add clippy rustfmt
RUN cargo install cargo-nextest --version "${NEXTEST_VERSION}" --locked

# Apt clean up
RUN apt autoremove -y \
    && apt clean \
    && rm -rf /var/lib/apt/lists/*

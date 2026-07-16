ARG RUST_VERSION=1.90
ARG ZAINO_IMAGE_TAG=0.3.0-rc.1
ARG ZEBRA_VERSION=6.0.0-rc.0
ARG NEXTEST_VERSION=0.9.128

FROM docker.io/zingodevops/zaino:${ZAINO_IMAGE_TAG} AS zainod-downloader
FROM docker.io/zfnd/zebra:${ZEBRA_VERSION} AS zebrad-downloader

FROM debian:trixie-slim AS runner

ARG RUST_VERSION
ARG ZAINO_IMAGE_TAG
ARG ZEBRA_VERSION
ARG NEXTEST_VERSION

LABEL org.zingolib.test-artifacts.versions.rust="${RUST_VERSION}"
LABEL org.zingolib.test-artifacts.versions.zaino="${ZAINO_IMAGE_TAG}"
LABEL org.zingolib.test-artifacts.versions.zebra="${ZEBRA_VERSION}"
LABEL org.zingolib.test-artifacts.versions.nextest="${NEXTEST_VERSION}"

# Copy regtest binaries
COPY --from=zainod-downloader /usr/local/bin/zainod /usr/bin/
COPY --from=zebrad-downloader /usr/local/bin/zebrad /usr/bin/

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
    protobuf-compiler \
    libclang-dev \
    ca-certificates \
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

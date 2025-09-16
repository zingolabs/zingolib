FROM debian:trixie-slim AS builder

WORKDIR /usr/src

# Install dependencies and update ca certificates
RUN apt update \
    && apt install -y --no-install-recommends --no-install-suggests \
    build-essential \
    pkg-config \
    libc6-dev \
    m4 \
    g++-multilib \
    autoconf \
    libtool \
    ncurses-dev \
    unzip \
    git \
    python3 \
    python3-zmq \
    zlib1g-dev \
    curl \
    bsdmainutils \
    automake \
    golang \
    ca-certificates \
    && update-ca-certificates

# Build lightwalletd
RUN git clone https://github.com/zcash/lightwalletd \
    && cd lightwalletd \
    && git checkout v0.4.18 \
    && make

# Build zcashd and fetch params
RUN git clone https://github.com/zcash/zcash.git \
    && cd zcash/ \
    && git checkout v6.3.0 \
    && ./zcutil/build.sh -j$(nproc)

FROM debian:trixie-slim AS runner

# Copy regtest binaries and zcash params from builder
COPY --from=builder /usr/src/lightwalletd/lightwalletd /usr/bin/
COPY --from=builder /usr/src/zcash/src/zcashd /usr/bin/
COPY --from=builder /usr/src/zcash/src/zcash-cli /usr/bin/

# Install dependencies and update ca certificates
RUN apt update \
    && apt install -y --no-install-recommends --no-install-suggests \
    python3 \
    git \
    gcc \
    curl \
    pkg-config \
    libssl-dev \
    build-essential \
    protobuf-compiler \
    libclang-dev \
    ca-certificates \
    && update-ca-certificates

# Install rust and cargo tarpaulin
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV HOME=/root
ENV CARGO_HOME=$HOME/.cargo
ENV RUSTUP_HOME=$HOME/.rustup
ENV PATH=$PATH:$CARGO_HOME/bin
RUN rustup toolchain install stable --profile minimal --component clippy \
    && rustup update \
    && rustup default stable
RUN cargo install cargo-nextest --locked
RUN cargo install cargo-tarpaulin

# Apt clean up
RUN apt autoremove -y \
    && apt clean \
    && rm -rf /var/lib/apt/lists/*


FROM debian:bookworm-slim

# Install dependencies
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
    libtinfo5 \
    ca-certificates \
    gcc \
    protobuf-compiler \
    libssl-dev \
    apt-transport-https \
    gnupg2 \
    procps \
    golang \
    && update-ca-certificates

# build lightwalletd
RUN  git clone https://github.com/zcash/lightwalletd && cd lightwalletd && make && \
     ln -s lightwalletd /usr/bin
# Install Rust and Cargo-Tarpaulin
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y
ENV HOME=/root
ENV CARGO_HOME=$HOME/.cargo
ENV PATH=$PATH:$CARGO_HOME/bin
RUN rustup toolchain install stable \
    && rustup component add clippy \
    && cargo install cargo-tarpaulin \
    && rustup update

# build zcashd and fetch params
RUN  git clone https://github.com/zcash/zcash.git \
     && cd zcash/ \
     && git checkout v5.4.2 \
     && ./zcutil/fetch-params.sh

RUN  cd zcash && ./zcutil/clean.sh && ./zcutil/build.sh -j$(nproc)
RUN  ln -s /zcash/src/zcashd /zcash/src/zcash-cli /usr/bin
RUN  rustup install stable && rustup default stable

WORKDIR /usr/src

RUN  mv -f /lightwalletd/lightwalletd /usr/bin
# Apt clean up
RUN apt autoremove -y \
    && apt clean \
    && rm -rf /var/lib/apt/lists/*

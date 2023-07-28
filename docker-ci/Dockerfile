FROM ubuntu:22.04 as builder

WORKDIR /usr/src/

# Install dependencies and update certificates
RUN apt update
RUN apt install -y --no-install-recommends --no-install-suggests \
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
    golang
RUN update-ca-certificates

# Build lightwalletd
RUN git clone https://github.com/zcash/lightwalletd \
    && cd lightwalletd \
    && git checkout v0.4.15 \
    && make

# Build zcashd and fetch params
RUN git clone https://github.com/zcash/zcash.git \
    && cd zcash/ \
    && git checkout v5.6.1 \
    && ./zcutil/fetch-params.sh \
    && ./zcutil/clean.sh \
    && ./zcutil/build.sh -j$(nproc)

FROM ubuntu:22.04

WORKDIR /usr/bin/

# Copy regtest binaries and zcash params from builder
COPY --from=builder /usr/src/lightwalletd/lightwalletd ./
COPY --from=builder /usr/src/zcash/src/zcashd ./
COPY --from=builder /usr/src/zcash/src/zcash-cli ./
COPY --from=builder /root/.zcash-params /root/

# Apt clean up
RUN apt autoremove -y \
    && apt clean \
    && rm -rf /var/lib/apt/lists/*

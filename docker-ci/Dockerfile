FROM debian:11.6

# Install dependencies
RUN apt update && \
    apt install curl build-essential gcc protobuf-compiler libssl-dev pkg-config git apt-transport-https gnupg2 unzip procps -y
    
# Set environmental variables
ENV CARGO_HOME=/usr/local/cargo
ENV CARGO_TARGET_DIR=/usr/local/build/target
ENV SCCACHE_DIR=/usr/local/sccache

# Install Rust and Cargo tools
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y
ENV PATH="/usr/local/cargo/bin:${PATH}"
RUN cargo install cargo-tarpaulin sccache
COPY /config.toml /usr/local/cargo/

# Install Zcash and fetch zero-knowledge parameters
RUN curl https://apt.z.cash/zcash.asc | gpg --import && \
    gpg --export 3FE63B67F85EA808DE9B880E6DEF3BAF272766C0 | apt-key add - && \
    echo "deb [arch=amd64] https://apt.z.cash/ bullseye main" | tee /etc/apt/sources.list.d/zcash.list && \
    apt update && apt install zcash && zcash-fetch-params

# Import lightwalletd binary
COPY /lightwalletd /usr/bin/

# Compile zingolib dependencies
WORKDIR /usr/src
RUN git clone -b dev --single-branch https://github.com/zingolabs/zingolib.git
WORKDIR zingolib
RUN cargo tarpaulin --no-run --all-features --verbose --workspace --avoid-cfg-tarpaulin --skip-clean --ignore-tests --release
WORKDIR /usr/src
RUN rm -rf zingolib/

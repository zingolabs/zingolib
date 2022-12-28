FROM debian:11.6

# Install packages
RUN apt update && \
    apt install curl build-essential gcc protobuf-compiler libssl-dev pkg-config git apt-transport-https gnupg2 unzip procps -y
    
# Install Rust and Cargo-Tarpaulin
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN cargo install cargo-tarpaulin

# Install Zcash and fetch zero-knowledge parameters
RUN curl https://apt.z.cash/zcash.asc | gpg --import && \
    gpg --export 3FE63B67F85EA808DE9B880E6DEF3BAF272766C0 | apt-key add - && \
    echo "deb [arch=amd64] https://apt.z.cash/ bullseye main" | tee /etc/apt/sources.list.d/zcash.list && \
    apt update && apt install zcash && zcash-fetch-params

# Import lightwalletd binary
COPY /lightwalletd /usr/bin/

# Set working directory
WORKDIR /usr/src

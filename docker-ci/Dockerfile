# Use latest stable debian release
FROM debian:latest

# Update and install dependencies
RUN apt update && apt upgrade -y && \
    apt install curl build-essential gcc make protobuf-compiler libssl-dev pkg-config git apt-transport-https wget gnupg2 unzip -y
    
# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Install Zcash and fetch zero-knowledge parameters
RUN wget -qO - https://apt.z.cash/zcash.asc | gpg --import && \
    gpg --export 3FE63B67F85EA808DE9B880E6DEF3BAF272766C0 | apt-key add - && \
    echo "deb [arch=amd64] https://apt.z.cash/ bullseye main" | tee /etc/apt/sources.list.d/zcash.list && \
    apt update && apt install zcash && zcash-fetch-params

# Install Lightwalletd 
# Building from source: docker in a docker issue
# Copying binary from host instead for now
COPY /bin/lightwalletd /usr/bin/

# WORKDIR /usr/src
# COPY /go/go1.19.3.linux-amd64.tar.gz ./go/
# RUN rm -rf /usr/local/go && tar -xzf ./go/go1.19.3.linux-amd64.tar.gz -C /usr/local 
# ENV PATH="$PATH:/usr/local/go/bin"
# RUN git clone https://github.com/zingolabs/lightwalletd.git
# WORKDIR lightwalletd
# RUN make

# Git clone zingolib (to be replaced with zingolib PR for CI)
WORKDIR /usr/src
RUN git clone https://github.com/zingolabs/zingolib.git

# Copy zcash and lightwallet binaries into zingolib regtest directory
WORKDIR zingolib
RUN cp /usr/bin/lightwalletd /usr/bin/zcashd /usr/bin/zcash-cli ./regtest/bin/

# Create CARGO_HOME directory for package caching
RUN mkdir ./cargo_home

RUN cargo build --release

RUN apt install netcat -y

# Cargo test
# ENTRYPOINT ["bash", "-c", "cargo test --release"]

# run container: 
# docker run --rm -e CARGO_HOME=/usr/src/zingolib/cargo_home -v crates-idx-vol:/usr/src/zingolib/cargo_home -it <image-name>
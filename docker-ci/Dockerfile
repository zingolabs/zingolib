# Parent image for zingolibs CI
FROM parent-ci:latest

# RUN git clone https://github.com/zingolabs/zingolib.git
RUN git clone https://github.com/Oscar-Pepper/zingolib.git # test only

# Copy zcash and lightwallet binaries into zingolib regtest directory
WORKDIR zingolib
RUN git checkout ci-update # test only
RUN cp /usr/bin/lightwalletd /usr/bin/zcashd /usr/bin/zcash-cli ./regtest/bin/

# Create CARGO_HOME directory for package caching
# RUN mkdir ./cargo_home

# ------------test only ----------------------
RUN cargo build --release
ENTRYPOINT ["bash", "-c", "./target/release/zingo-cli --regtest"]
#---------------------------------------------

# Cargo test
# ENTRYPOINT ["bash", "-c", "cargo test --release"]

# caching crate index
# docker run --rm -e CARGO_HOME=/usr/src/zingolib/cargo_home -v crates-idx-vol:/usr/src/zingolib/cargo_home -it <image-name>

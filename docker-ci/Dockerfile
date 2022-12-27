# Parent image for zingolibs CI
FROM builder:latest

RUN git clone https://github.com/zingolabs/zingolib.git

# Copy zcash and lightwallet binaries into zingolib regtest directory
WORKDIR zingolib
RUN ln -s /usr/bin/lightwalletd /usr/bin/zcashd /usr/bin/zcash-cli ./regtest/bin/

# Cargo test
# ENTRYPOINT ["bash", "-c", "cargo test --release --features cross_version"]

# Cargo tarpaulin
ENTRYPOINT ["bash", "-c", "cargo tarpaulin --all-features --avoid-cfg-tarpaulin --skip-clean --verbose --workspace --ignore-tests --release --timeout 300 --out Xml"]


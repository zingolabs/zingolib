# Parent image for zingolibs CI
FROM builder-root:latest

RUN git clone https://github.com/zingolabs/zingolib.git

# Copy zcash and lightwallet binaries into zingolib regtest directory
WORKDIR zingolib
RUN ln -s /usr/bin/lightwalletd /usr/bin/zcashd /usr/bin/zcash-cli ./regtest/bin/

# Cargo test
# ENTRYPOINT ["bash", "-c", "cargo test --release --features cross_version"]

# Cargo tarpaulin
ENTRYPOINT ["bash", "-c", "cargo tarpaulin --all-features --verbose --workspace --avoid-cfg-tarpaulin --skip-clean --ignore-tests --release --timeout 300 --out xml"]

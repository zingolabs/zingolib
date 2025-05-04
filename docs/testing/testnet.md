cargo nextest run --run-ignored=all orchard_glory_goddess
cargo nextest run --run-ignored=all sapling_glory_goddess


to update the testnet wallet run
cargo run -- -c=testnet --server="https://lightwalletd.testnet.electriccoin.co:9067" --data-dir=zingolib/src/wallet/disk/testing/examples/testnet/glory_goddess/latest

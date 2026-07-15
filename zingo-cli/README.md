# Zingo CLI

A command-line interface for the Zingo wallet.

## Building

To build the zingo-cli binary from the workspace:

```bash
cargo build --release -p zingo-cli
```

The binary will be available at `target/release/zingo-cli`.

## Running

By default, zingo-cli stores wallet data in a `wallets/` directory in the current working directory.

The `--chain` argument allows you to select which network to connect to. If not specified, it defaults to mainnet.

### Mainnet

To connect to mainnet (default):

```bash
# Uses default wallet location: ./wallets/
./target/release/zingo-cli

# Or explicitly specify mainnet:
./target/release/zingo-cli --chain mainnet

# Or specify a custom data directory:
./target/release/zingo-cli --data-dir /path/to/mainnet-wallet
```

### Testnet

To connect to testnet:

```bash
# Uses default wallet location: ./wallets/
./target/release/zingo-cli --chain testnet

# Or specify a custom data directory:
./target/release/zingo-cli --chain testnet --data-dir /path/to/testnet-wallet
```

### Regtest Mode

To run in regtest mode:
1. Build the zingo-cli binary.
2. Launch a local network: a `zebrad` validator with a `zainod` indexer in
   front of it (the Core stack). The `zcash_local_net` crate in the
   infrastructure repo launches and manages the pair:
   https://github.com/zingolabs/infrastructure/tree/dev/zcash_local_net
3. Create a wallet directory (data-dir) and run zingo-cli against the
   indexer's URI:
```bash
./target/release/zingo-cli --chain regtest --server 127.0.0.1:8137 --data-dir ~/tmp/regtest_temp
```

## Exiting the CLI

To quit the Zingo CLI, use the `quit` command (not `exit`).

**Note:** Each network (mainnet, testnet, regtest) requires its own wallet data. If you get an error about wallet chain name mismatch, ensure you're using the correct data directory for your chosen network.

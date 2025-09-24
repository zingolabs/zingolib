# Zingo CLI

A command-line interface for the Zingo wallet.

## Building

### Default Build (Mainnet/Testnet)

To build the standard zingo-cli binary that works with mainnet and testnet:

```bash
cargo build --release
```

The binary will be available at `target/release/zingo-cli`.

### Build with Regtest Support

To build zingo-cli with regtest support in addition to mainnet and testnet:

```bash
cargo build --release --features regtest
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

To run in regtest mode (requires building with the `regtest` feature):

```bash
./target/release/zingo-cli --chain regtest
```

This will:
- Launch a local regtest network (zcashd and lightwalletd)
- Start the network on port 17555
- Create a new, temporary wallet automatically

**Note:** Each network (mainnet, testnet, regtest) requires its own wallet data. If you get an error about wallet chain name mismatch, ensure you're using the correct data directory for your chosen network.

## Exiting the CLI

To quit the Zingo CLI, use the `quit` command (not `exit`).

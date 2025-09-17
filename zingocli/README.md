# Zingo CLI

A command-line interface for the Zingo wallet.

## Building

### Default Build (Mainnet/Testnet)

To build the standard zingo-cli binary that works with mainnet and testnet:

```bash
cargo build --release
```

The binary will be available at `target/release/zingo-cli`.

### Regtest Build

To build a special binary for regtest mode, compile the purpose-built `zingo-cli-regtest` binary:

```bash
cargo build --release -p zingo-cli --features regtest --bin zingo-cli-regtest
```

The binary will be available at `target/release/zingo-cli-regtest`.

## Running

### Mainnet

To connect to mainnet:

```bash
./target/release/zingo-cli
```

### Testnet

To connect to testnet, use the `--chain` flag:

```bash
./target/release/zingo-cli --chain testnet
```

**Note:** If you have an existing wallet created for a different network (mainnet), you'll need to either:
- Use a different data directory: `./target/release/zingo-cli --chain testnet --data-dir /path/to/testnet-wallet`
- Or remove/rename your existing wallet directory before switching networks

### Regtest Mode

To run in regtest mode, use the specially compiled regtest binary:

```bash
./target/release/zingo-cli-regtest
```

This will:
- Launch a local regtest network (zcashd and lightwalletd)
- Start the network on port 17555
- Create a new wallet automatically

Note: The regtest binary is purpose-built with only the necessary dependencies for regtest operation, as indicated by the project's dependency elision approach.

## Exiting the CLI

To quit the Zingo CLI, use the `quit` command (not `exit`).

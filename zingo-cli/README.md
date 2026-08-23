# Zingo CLI

A command-line light wallet for Zcash. `zingo-cli` either runs a single
command and exits, or — given no command — starts an interactive prompt.

This document catalogs **every way the CLI can be launched**: how to build it,
the launchers that run it, the two modes of operation, the connectivity model
that a launch selects, and the session options and environment variables that
shape a session.

---

## Table of contents

- [Building](#building)
- [Ways to launch](#ways-to-launch)
  - [1. The compiled binary directly](#1-the-compiled-binary-directly)
  - [2. `cargo run`](#2-cargo-run)
  - [3. `makers run-cli` (recommended for development)](#3-makers-run-cli-recommended-for-development)
  - [Using the mixnet without cargo-make](#using-the-mixnet-without-cargo-make)
  - [4. Docker](#4-docker)
- [Modes of operation](#modes-of-operation)
  - [Interactive mode (the REPL)](#interactive-mode-the-repl)
  - [Command mode (one-shot)](#command-mode-one-shot)
- [Connectivity: offline-first, consent to go online](#connectivity-offline-first-consent-to-go-online)
- [Choosing a network](#choosing-a-network)
  - [Mainnet](#mainnet)
  - [Testnet](#testnet)
  - [Regtest](#regtest)
- [Creating or restoring a wallet](#creating-or-restoring-a-wallet)
- [Session options reference](#session-options-reference)
- [Environment variables](#environment-variables)
- [Build features that change how it launches](#build-features-that-change-how-it-launches)
- [Exiting the CLI](#exiting-the-cli)
- [Troubleshooting](#troubleshooting)

---

## Building

Build the binary from the workspace root:

```bash
# Release build (recommended for real use)
cargo build --release -p zingo-cli
# Binary: ./target/release/zingo-cli

# Debug build (faster to compile)
cargo build -p zingo-cli
# Binary: ./target/debug/zingo-cli
```

By default the build compiles in the **mixnet (Nym) transport** — this is the
`nym` default feature. See
[Build features](#build-features-that-change-how-it-launches) for how to opt out
and what changes when you do.

---

## Ways to launch

There are four ways to run the CLI. They differ only in *how the binary is
produced and invoked* — every one of them accepts the same
[session options](#session-options-reference) and
[commands](#command-mode-one-shot).

### 1. The compiled binary directly

After [building](#building), run the binary and pass options/commands directly:

```bash
# Start the interactive prompt (no command given)
./target/release/zingo-cli

# Run a single command and exit
./target/release/zingo-cli addresses

# With session options
./target/release/zingo-cli --data-dir ~/my-wallet --chain testnet
```

Print the version or the full help without starting a session:

```bash
./target/release/zingo-cli --version
./target/release/zingo-cli --help
./target/release/zingo-cli help          # same command surface, live posture
./target/release/zingo-cli help send     # help for one command
```

### 2. `cargo run`

Build (if needed) and run in one step. Everything after `--` is forwarded to the
CLI verbatim:

```bash
# Interactive prompt
cargo run -p zingo-cli

# One-shot command
cargo run -p zingo-cli -- addresses

# Release profile + session options
cargo run --release -p zingo-cli -- --chain testnet --data-dir ~/testnet-wallet

# Build without the mixnet transport (see Build features)
cargo run -p zingo-cli --no-default-features -- info
```

### 3. `makers run-cli` (recommended for development)

The workspace ships a [cargo-make](https://github.com/sagiegurari/cargo-make)
task that builds the CLI **and** bundles the `nym-proxy` binary beside it, so the
CLI's provisioning search finds the proxy automatically when a session goes
online. Install `cargo-make` once (`cargo install cargo-make`), then:

```bash
# Build (with the mixnet default), bundle nym-proxy, and launch
makers run-cli

# Forward any session option / command to zingo-cli
makers run-cli --chain testnet
makers run-cli addresses
makers run-cli --data-dir ~/my-wallet --online

# Debug build (the default is a release build)
makers run-cli --debug

# Opt out of the mixnet default: a plain build, nothing bundled
makers run-cli --clearnet
```

Notes:

- `--clearnet` and `--debug` are consumed by the launcher; **every other
  argument is forwarded to `zingo-cli` unchanged.**
- This task never launches the proxy itself. The CLI owns that lifecycle: it
  spawns the proxy only at an online session's go-online moment, and an offline
  session boots no proxy at all.
- Launching with `makers run-cli` does **not** imply consent to go online. The
  session is offline until a consent act (see
  [Connectivity](#connectivity-offline-first-consent-to-go-online)).

### Using the mixnet without cargo-make

You do **not** need cargo-make to run with the mixnet. The mixnet transport is
the `nym` **default feature**, so a plain `cargo build -p zingo-cli` already
compiles it in. The only thing `makers run-cli` adds is building and placing the
`nym-proxy` binary — and that binary is **never** produced by
`cargo build -p zingo-cli`, because it lives in the separate `zingo-netutils`
workspace (its own lockfile, ADR 0011). Do those two steps by hand:

```bash
# 1. Build the CLI — the mixnet transport is already on by default.
cargo build --release -p zingo-cli

# 2. Build the nym-proxy binary from the zingo-netutils workspace.
cargo build --release \
  --manifest-path zingo-netutils/Cargo.toml \
  --features nym --bin nym-proxy
# → produces zingo-netutils/target/release/nym-proxy
```

Then launch online and tell the CLI where the proxy is, using **any one** of the
resolution methods (precedence: `--nym-proxy` → `$ZINGO_NYM_PROXY` → a
`nym-proxy` beside the CLI binary → `nym-proxy` on `PATH`):

```bash
# a) Explicit flag
./target/release/zingo-cli --online \
  --nym-proxy zingo-netutils/target/release/nym-proxy

# b) Environment variable
export ZINGO_NYM_PROXY="$PWD/zingo-netutils/target/release/nym-proxy"
./target/release/zingo-cli --online

# c) Copy it beside the CLI binary — then it's found with no configuration
cp zingo-netutils/target/release/nym-proxy target/release/
./target/release/zingo-cli --online
```

Compiling `nym` in starts nothing on its own: the proxy is spawned only when the
session goes online, so a consent act (`--online` above, or `--server` /
`--remember-online` / in-session `network on`) is still required. For a debug
build, drop `--release` from both `cargo build` commands and use
`target/debug/…` paths throughout.

### 4. Docker

The repository builds a container image that carries the `zingo-cli` binary. The
image's entrypoint prints the version, creates a wallet on first run (syncing and
printing an address), prints server info, and then execs whatever command you
pass:

```bash
# Interactive session against a chosen server
docker run -it zingo-cli:latest ./zingo-cli --server https://zec.rocks:443

# One-shot command inside the container
docker run -it zingo-cli:latest ./zingo-cli --nosync info

# No arguments → the entrypoint runs, then the default CMD prints --help
docker run -it zingo-cli:latest
```

The reproducible StageX image can be built with `make` from the repository root
and loaded into Docker with `make load`; see the top-level
[README](../README.md) for that pipeline.

---

## Modes of operation

The CLI selects its mode from the arguments at parse time:

- **No command** on the command line → the **interactive prompt**.
- **A command** on the command line → **command mode**: run it and exit.

Both modes use the same command grammar, so `zingo-cli balance` and typing
`balance` at the prompt do the same thing.

### Interactive mode (the REPL)

Launch with no trailing command:

```bash
./target/release/zingo-cli
```

You get a prompt that shows the chain, the wallet's block height, and sync
status, for example:

```
(main) Block:2500000 [Synced 1200 / 1200 outputs] >>
```

Type `help` to list commands, or `help <command>` for one command's detail. In
interactive mode, tracing/log output is written to a **log file** (default
`.zingo-cli/cli.log`, override with `--log-file`) so it does not clutter the
prompt.

### Command mode (one-shot)

Pass a command as an argument; the CLI runs it and exits with a status code
(`0` on success, non-zero on failure). In command mode, log output goes to
stderr instead of a file.

```bash
./target/release/zingo-cli addresses
./target/release/zingo-cli --waitsync balance
./target/release/zingo-cli send <address> <zatoshis> "optional memo"
```

Useful one-shot examples:

- `zingo-cli addresses` — list the wallet's addresses and exit.
- `zingo-cli --waitsync balance` — block until the background sync completes,
  then print balances (sync-dependent commands usually want `--waitsync`).
- `zingo-cli --nosync info` — query the indexer's info without syncing first.

Run `zingo-cli help` (or `zingo-cli --help`) to see the full command list; the
set adapts to the session's live posture (an offline session does not list
network-requiring commands).

---

## Connectivity: offline-first, consent to go online

**A fresh launch is offline by design.** Whether a session touches the network is
decided at launch from your consent acts and any stored standing choice. Local
operations (addresses, balances, history, proposing) always work offline; sync,
sending, and server commands require going online.

A session goes **online** if any of these is true:

| Launch act | Effect |
| --- | --- |
| `--online` | Consent for **this session only**; the choice is not persisted. |
| `--remember-online` | Consent for this session **and** store a standing consent beside the wallet, so future sessions attach automatically. |
| `--server <uri>` | Pinning an explicit indexer implies consent to go online. |
| A stored standing consent | A previous `--remember-online` keeps future sessions online automatically. |
| In-session `network on` | Grants consent mid-session and switches to online mode. |

Otherwise the session runs **offline**. You can make offline explicit and silence
the first-boot notice with `--offline` (a deliberate zero-traffic session that no
in-session command can lift). Remove a stored standing consent with
`--forget-online`.

```bash
# This session only, online:
./target/release/zingo-cli --online

# Go online now and remember it for next time:
./target/release/zingo-cli --remember-online

# Deliberately offline (no network, ever, this session):
./target/release/zingo-cli --offline

# Drop a stored standing consent, then run (offline unless re-consented):
./target/release/zingo-cli --forget-online
```

Conflicts enforced by the parser: `--offline` cannot combine with `--server`,
`--waitsync`, or `--online`; `--remember-online` cannot combine with `--offline`
or `--forget-online`. Passing `--online` with a one-shot command that needs no
network (e.g. `--online addresses`) is refused as a contradiction.

When online **without** a pinned `--server`, the session runs a
Server-Selection Sweep that picks the sync indexer for you. Pin one explicitly
with `--server` to skip the sweep's substitution.

---

## Choosing a network

The `--chain` (`-c`) option selects the network: `mainnet` (default),
`testnet`, or `regtest`. Each network needs **its own wallet data directory** —
mixing them raises a chain-name mismatch error.

By default, wallet data is stored in a `wallets/` directory under the current
working directory. Override with `--data-dir`.

### Mainnet

```bash
# Default (mainnet)
./target/release/zingo-cli

# Explicit, online, with a dedicated data dir
./target/release/zingo-cli --chain mainnet --online --data-dir ~/mainnet-wallet
```

### Testnet

```bash
./target/release/zingo-cli --chain testnet --online --data-dir ~/testnet-wallet
```

### Regtest

Regtest runs against a local network you launch yourself:

1. Build the `zingo-cli` binary.
2. Launch a local network — a `zebrad` validator with a `zainod` indexer in
   front of it (the Core stack). The `zcash_local_net` crate in the
   infrastructure repo launches and manages the pair:
   <https://github.com/zingolabs/infrastructure/tree/dev/zcash_local_net>
3. Run the CLI against the indexer's URI (pinning `--server` implies online
   consent, so no extra flag is needed):

```bash
./target/release/zingo-cli \
  --chain regtest \
  --server 127.0.0.1:8137 \
  --data-dir ~/tmp/regtest_temp
```

A `--server` value without a scheme is normalized to `http://…`, and a value
without a port has `:9067` appended.

---

## Creating or restoring a wallet

If the data directory has no wallet, one is created on launch. You control what
gets created:

```bash
# New wallet (created automatically if none exists in the data dir)
./target/release/zingo-cli --data-dir ~/new-wallet

# Restore from a 12/15/18/21/24-word seed phrase (needs a birthday)
./target/release/zingo-cli \
  --seed "twenty four word seed phrase ..." \
  --birthday 600000 \
  --data-dir ~/restored-wallet

# Restore watch-only from a Unified Full Viewing Key (needs a birthday)
./target/release/zingo-cli \
  --viewkey <UFVK> \
  --birthday 600000 \
  --data-dir ~/watch-only
```

- `--birthday` is the earliest block height where the wallet has a transaction.
  If you don't know it, `--birthday 0` scans from the start of the chain (slow).
- Restoring with `--seed`/`--viewkey` **fails if a wallet already exists** in the
  data dir; use a fresh `--data-dir` or move the existing wallet aside.
- **Avoid putting a seed on the command line** — it is visible in the host's
  process list and shell history. Export it in the `ZINGO_SEED` environment
  variable instead (see below); the `--seed` flag takes precedence when both are
  present.

---

## Session options reference

Session options configure the whole session and must appear **before** any
command. (The CLI detects a misplaced session option after a command and tells
you the corrected invocation.)

| Option | Description |
| --- | --- |
| `-n`, `--nosync` | Don't auto-sync at startup. |
| `--waitsync` | Block the command until the background sync completes (no effect with `--nosync`). |
| `-c`, `--chain <CHAIN>` | `mainnet` (default), `testnet`, or `regtest`. |
| `-s`, `--seed <PHRASE>` | Create a new wallet from a 12/15/18/21/24-word seed. |
| `--viewkey <UFVK>` | Create a new wallet from an encoded unified full viewing key. |
| `--birthday <HEIGHT>` | Wallet birthday (earliest block height with a transaction) when restoring. |
| `--server <URI>` | Pin a specific indexer server (also implies online consent). |
| `--offline` | Deliberate offline session; no indexer is ever configured. |
| `--online` | Consent to go online for this session only (not persisted). |
| `--remember-online` | Consent to go online and store a standing consent for future sessions. |
| `--forget-online` | Remove the stored standing consent before deciding connectivity. |
| `--nym-proxy <PATH>` | Path to the `nym-proxy` binary for Mixnet Mode (`nym` builds only). |
| `--indexer-diary` | Record per-indexer send/probe outcomes this session (needs the `nym-diary` build feature). |
| `--data-dir <PATH>` | Data directory for wallet + logs (default: `./wallets`). |
| `--log-file <PATH>` | Log file path for interactive mode (default: `.zingo-cli/cli.log`). |
| `-V`, `--version` | Print the version and exit. |
| `-h`, `--help` | Print help and exit. |

---

## Environment variables

| Variable | Effect |
| --- | --- |
| `ZINGO_SEED` | Supplies the wallet seed phrase without putting it in the process list or shell history. The `--seed` flag overrides it. |
| `ZINGO_NYM_PROXY` | Path to the `nym-proxy` binary. Consulted before the bundled/`PATH` proxy when `--nym-proxy` isn't given. |
| `ZINGO_DISABLE_SAVER` | If set, the save task does not run and **nothing persists this session**. |
| `RUST_LOG` | Standard tracing filter (e.g. `RUST_LOG=info`), applied to the log destination for the session's mode. |

---

## Build features that change how it launches

| Feature | Default | Effect on launch |
| --- | --- | --- |
| `nym` | **on** | Compiles in the mixnet transport, so a session can go online. Opting out (`--no-default-features`, or `makers run-cli --clearnet`) makes **Offline Mode the only mode**: the online consent acts refuse loudly and a stored standing consent is reported as inert. |
| `nym-diary` | off | Enables the on-disk indexer diary so `--indexer-diary` records and `network history` displays it. Without it, `--indexer-diary` warns and records nothing. |
| `clearnet-test-mode` | off | Re-enables the quarantined clearnet server-selection sweep. A deliberate, review-gated test build — never for ordinary use. |

Build with features explicitly, for example:

```bash
# Clearnet-only build (no mixnet capability)
cargo build --release -p zingo-cli --no-default-features

# Enable the indexer diary
cargo build --release -p zingo-cli --features nym-diary
```

---

## Exiting the CLI

At the interactive prompt, quit with the `quit` command (not `exit`). `Ctrl-C`
and `Ctrl-D` also end the session.

---

## Troubleshooting

- **"wallet chain name mismatch"** — the data directory holds a wallet for a
  different network. Use a separate `--data-dir` per network (mainnet, testnet,
  regtest).
- **A network command is refused as offline** — the session has no connectivity
  consent. Grant it for this session with `--online` (or `network on` at the
  prompt), or `--remember-online` to persist it.
- **`--indexer-diary` "has no effect"** — the binary was built without the
  `nym-diary` feature. Rebuild with `--features nym-diary`.
- **Going online refused with "no mixnet capability"** — the binary was built
  with `--no-default-features` (clearnet-only). Rebuild with default features
  (plain `cargo build`, or `makers run-cli`) to go online.
- **A session option after the command is rejected** — session options must come
  before the command; the CLI prints the corrected invocation.

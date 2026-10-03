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
  - [Prerequisites](#prerequisites)
  - [The build procedure](#the-build-procedure)
- [Ways to launch](#ways-to-launch)
  - [1. `makers run-cli`](#1-makers-run-cli)
  - [2. The built binary directly](#2-the-built-binary-directly)
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

One procedure builds the CLI: the workspace's
[cargo-make](https://github.com/sagiegurari/cargo-make) task `run-cli`. The task
compiles `zingo-cli` with the mixnet (Nym) transport, builds the `nym-proxy`
binary, and places the proxy beside the CLI, where a session that goes online
finds it.

The proxy is the reason for the task. `nym-proxy` lives in the separate
`zingo-netutils` workspace, which keeps its own lockfile (ADR 0011), so a bare
`cargo build -p zingo-cli` never produces it, and a CLI that finds no proxy
cannot go online.

### Prerequisites

- **Rust**, installed through [rustup](https://rustup.rs). The repository's
  `rust-toolchain.toml` pins the toolchain, and rustup installs it on the first
  build.
- **Build tools** for your platform. On Ubuntu, run
  `sudo apt install build-essential gcc libsqlite3-dev`.
- **The protobuf compiler.** On Ubuntu, run
  `sudo apt install protobuf-compiler`.
- **cargo-make**, which provides the `makers` command. Run
  `cargo install cargo-make`.

### The build procedure

```bash
git clone https://github.com/zingolabs/zingolib.git
cd zingolib

# Build the release CLI, bundle nym-proxy beside it, and stop
makers run-cli --build-only
# Binaries: ./target/release/zingo-cli and ./target/release/nym-proxy
```

The launcher consumes these flags itself, wherever they appear:

| Launcher flag | Effect |
| --- | --- |
| `--build-only` | Stop after the build and the bundling; launch no session. |
| `--debug` | Build the debug profile instead of the release profile. The binaries land in `./target/debug/`. |
| `--features <list>` | Add cargo features to the build (see [Build features](#build-features-that-change-how-it-launches)). |
| `--target-dir <dir>` | Build into the named directory, so two differently-featured builds stand side by side without rebuilding each other. |

Without `--build-only`, the task goes on to launch the CLI, as the next section
describes.

---

## Ways to launch

There are two ways to run the CLI, and both run the binary the
[build procedure](#the-build-procedure) produces. Each accepts the same
[session options](#session-options-reference) and
[commands](#command-mode-one-shot).

### 1. `makers run-cli`

The task rebuilds whatever changed, bundles the proxy, and launches the CLI in
one step. It forwards every argument that is not a launcher flag to `zingo-cli`
unchanged:

```bash
# Build, bundle nym-proxy, and start the interactive prompt
makers run-cli

# Forward any session option / command to zingo-cli
makers run-cli --chain testnet
makers run-cli addresses
makers run-cli --data-dir ~/my-wallet --online
```

Notes:

- This task never launches the proxy itself. The CLI owns that lifecycle: it
  spawns the proxy only at an online session's go-online moment, and an offline
  session boots no proxy at all.
- Launching with `makers run-cli` does **not** imply consent to go online. The
  session is offline until a consent act (see
  [Connectivity](#connectivity-offline-first-consent-to-go-online)).

### 2. The built binary directly

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

The CLI resolves the proxy by this precedence: `--nym-proxy`, then
`$ZINGO_NYM_PROXY`, then a `nym-proxy` beside the CLI binary, then `nym-proxy`
on `PATH`. The build places the proxy beside the binary, so the built pair needs
no configuration. If you move the CLI elsewhere, move `nym-proxy` with it, or
name the proxy's location with the flag or the variable.

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
with `--server` to skip the sweep's substitution. To run an indexer of your own
and pin it, see [zaino](https://github.com/zingolabs/zaino).

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

1. [Build](#building) the `zingo-cli` binary.
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
| `nym` | **on** | Compiles in the mixnet transport, so a session can go online. Opting out (`makers run-cli --nakednet`) builds without the transport and bundles no proxy, which makes **Offline Mode the only mode**: the online consent acts refuse loudly and a stored standing consent is reported as inert. |
| `nakednet-test-mode` | off | Re-enables the quarantined nakednet server-selection sweep. A deliberate, review-gated test build — never for ordinary use. |

The [build procedure](#the-build-procedure) selects features through its
launcher flags, for example:

```bash
# Nakednet-only build (no mixnet capability, no proxy bundled)
makers run-cli --nakednet --build-only

# Add cargo features to the build
makers run-cli --features <list> --build-only
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
- **Going online refused with "no mixnet capability"** — the binary was built
  nakednet-only (`makers run-cli --nakednet`). Rebuild with the
  [build procedure](#the-build-procedure), without that flag, to go online.
- **A session option after the command is rejected** — session options must come
  before the command; the CLI prints the corrected invocation.

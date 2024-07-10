## Zingolib
[![license](https://img.shields.io/github/license/zingolabs/zingolib)](LICENSE) [![codecov](https://codecov.io/gh/zingolabs/zingolib/branch/dev/graph/badge.svg?token=WMKTJMQY28)](https://codecov.io/gh/zingolabs/zingolib)
This repo provides both a library for zingo-mobile, as well as an included cli application to interact with zcashd via lightwalletd.

# Security Vulnerability Disclosure

If you believe you have discovered a security issue, please contact us at:

zingodisclosure@proton.me

## Zingo CLI
`zingo-cli` is a command line lightwalletd-proxy client. To use it, see "compiling from source" below. Releases are currently only provisional, we will update the README as releases come out.

## Privacy
* While all the keys and transaction detection happens on the client, the server can learn what blocks contain your shielded transactions.
* The server also learns other metadata about you like your ip address etc...
* Also remember that t-addresses are publicly visible on the blockchain.
* Price information is retrieved from Gemini exchange.

### Note Management
Zingo-CLI does automatic note and utxo management, which means it doesn't allow you to manually select which address to send outgoing transactions from. It follows these principles:
* Defaults to sending shielded transactions, even if you're sending to a transparent address
* Can select funds from multiple shielded addresses in the same transaction
* Will automatically shield your sapling funds at the first opportunity
    * When sending an outgoing transaction to a shielded address, Zingo-CLI can decide to use the transaction to additionally shield your sapling funds (i.e., send your sapling funds to your own orchard address in the same transaction)
* Transparent funds are only spent via explicit shield operations

## Compiling from source

#### Pre-requisites
* Rust v1.79 or higher.
    * Run `rustup update` to get the latest version of Rust if you already have it installed
* Rustfmt
    * Run `rustup component add rustfmt` to add rustfmt
* Build Tools
    * Please install the build tools for your platform. On Ubuntu `sudo apt install build-essential gcc`
* Protobuf Compiler
    * Please install the protobuf compiler for your platform. On Ubuntu `sudo apt install protobuf-compiler`
* OpenSSL Dev
    * Please install development packages of openssl. On Ubuntu `sudo apt install libssl-dev`

```
git clone https://github.com/zingolabs/zingolib.git
cd zingolib
cargo build --release --package zingo-cli
./target/release/zingo-cli --data-dir /path/to/data_directory/
```

This will launch the interactive prompt. Type `help` to get a list of commands.

## Notes:
* If you want to run your own server, please see [zingo lightwalletd](https://github.com/zingolabs/lightwalletd), and then run `./zingo-cli --server http://127.0.0.1:9067`
* The default log file is in `~/.zcash/zingo-wallet.debug.log`. A default wallet is stored in `~/.zcash/zingo-wallet.dat`
* Currently, the default, hard-coded `lightwalletd` server is https://mainnet.lightwalletd.com:9067/. To change this, you can modify the `DEFAULT_SERVER` const in `config/src/lib.rs`

## Running in non-interactive mode:
You can also run `zingo-cli` in non-interactive mode by passing the command you want to run as an argument. For example, `zingo-cli addresses` will list all wallet addresses and exit.
Run `zingo-cli help` to see a list of all commands.

## Options
Here are some CLI arguments you can pass to `zingo-cli`. Please run `zingo-cli --help` for the full list.

* `--data-dir`: uses the specified path as data directory. This is required when not using the `--regtest` option.
    * Example: `./zingo-cli --data-dir /path/to/data_directory/` will use the provided directory to store `zingo-wallet.dat` and logs. If the provided directory does not exist, it will create it.
* `--server`: Connect to a custom zcash lightwalletd server.
    * Example: `./zingo-cli --data-dir /path/to/data_directory/ --server 127.0.0.1:9067`
* `--seed`: Restore a wallet from a seed phrase. Note that this will fail if there is an existing wallet. Delete (or move) any existing wallet to restore from the 24-word seed phrase
* `--birthday`: Specify wallet birthday when restoring from seed. This is the earliest block height where the wallet has a transaction.
    * Example: `./zingo-cli --data-dir /path/to/data_directory/ --seed "twenty four words seed phrase" --birthday 1234567`
* `--recover`: Attempt to recover the seed phrase from a corrupted wallet

## Regtest
There is an experimental feature flag available with `zingo-cli`, in which the cli works in regtest mode, by also locally running `zcashd` and `lightwalletd`.

For a relatively recent user experience please see: https://free2z.cash/zingo-cli-in-regtest-mode

Please see `docs/TEST_REGTEST.md` for a detailed explanation.

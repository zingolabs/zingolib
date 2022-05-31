## Zecwalletlitelib
[![license](https://img.shields.io/github/license/zingolabs/zecwalletlitelib)](LICENSE) [![codecov](https://codecov.io/gh/zingolabs/zecwalletlitelib/branch/dev/graph/badge.svg?token=WMKTJMQY28)](https://codecov.io/gh/zingolabs/zecwalletlitelib)
This repo provides both a library for zecwallet-lite and zecwallet-mobile, as well as an included cli application to interact with zcashd via lightwalletd.

## WARNING! Experimental
* This is experimental software. 
* It should not be used in production! 
* It should not be relied upon for custody, transfer, or receipt of ZEC!
* Please use only if you know specifically what you are doing!!

## WARNING! Lightwallet
* Using this software (a light wallet) does not offer the full privacy or security of running a full-node zcash client.
* This software does not provide privacy guarantees against network monitoring of the type or pattern of traffic it generates. That is to say, in some cases, the specifics of use may be able to remain private, but the use of this tool may be apparent to network observers.

## Zecwallet CLI
`zecwallet-cli` is a command line ZecWallet light client. To use it, see "compiling from source" below. Releases are currently only provisional, we will update the README as releases come out.

## Privacy 
* While all the keys and transaction detection happens on the client, the server can learn what blocks contain your shielded transactions.
* The server also learns other metadata about you like your ip address etc...
* Also remember that t-addresses don't provide any privacy protection.

### Note Management
Zecwallet-CLI does automatic note and utxo management, which means it doesn't allow you to manually select which address to send outgoing transactions from. It follows these principles:
* Defaults to sending shielded transactions, even if you're sending to a transparent address
* Sapling funds need at least 5 confirmations before they can be spent
* Can select funds from multiple shielded addresses in the same transaction
* Will automatically shield your transparent funds at the first opportunity
    * When sending an outgoing transaction to a shielded address, Zecwallet-CLI can decide to use the transaction to additionally shield your transparent funds (i.e., send your transparent funds to your own shielded address in the same transaction)

## Compiling from source

#### Pre-requisites
* Rust v1.37 or higher.
    * Run `rustup update` to get the latest version of Rust if you already have it installed
* Rustfmt
    * Run `rustup component add rustfmt` to add rustfmt
* Build tools
    * Please install the build tools for your platform. On Ubuntu `sudo apt install build-essential gcc`

```
git clone https://github.com/zingolabs/zecwalletlitelib.git
cargo build --release
./target/release/zecwallet-cli
```

This will launch the interactive prompt. Type `help` to get a list of commands.

## Notes:
* If you want to run your own server, please see [zecwallet lightwalletd](https://github.com/zingolabs/lightwalletd), and then run `./zecwallet-cli --server http://127.0.0.1:9067`
* The log file is in `~/.zcash/zecwallet-light-wallet.debug.log`. Wallet is stored in `~/.zcash/zecwallet-light-wallet.dat`
* Currently, the default, hard-coded `lightwalletd` server is https://lwdv3.zecwallet.co:443/. To change this, you can modify line 25 of `lib/src/lightclient/lightclient_config.rs`

## Running in non-interactive mode:
You can also run `zecwallet-cli` in non-interactive mode by passing the command you want to run as an argument. For example, `zecwallet-cli addresses` will list all wallet addresses and exit. 
Run `zecwallet-cli help` to see a list of all commands. 

## Options
Here are some CLI arguments you can pass to `zecwallet-cli`. Please run `zecwallet-cli --help` for the full list. 

* `--server`: Connect to a custom zecwallet lightwalletd server. 
    * Example: `./zecwallet-cli --server 127.0.0.1:9067`
* `--seed`: Restore a wallet from a seed phrase. Note that this will fail if there is an existing wallet. Delete (or move) any existing wallet to restore from the 24-word seed phrase
    * Example: `./zecwallet-cli --seed "twenty four words seed phrase"`
 * `--recover`: Attempt to recover the seed phrase from a corrupted wallet

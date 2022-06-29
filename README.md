## Zingolib
[![license](https://img.shields.io/github/license/zingolabs/zingolib)](LICENSE) [![codecov](https://codecov.io/gh/zingolabs/zingolib/branch/dev/graph/badge.svg?token=WMKTJMQY28)](https://codecov.io/gh/zingolabs/zingolib)
This repo provides both a library for zingoproxyclient and zingo-mobile, as well as an included cli application to interact with zcashd via lightwalletd.

## WARNING! Experimental
* This is experimental software. 
* It should not be used in production! 
* It should not be relied upon for custody, transfer, or receipt of ZEC!
* Please use only if you know specifically what you are doing!!

## WARNING! Lightwallet
* Using this software (a light wallet) does not offer the full privacy or security of running a full-node zcash client.
* This software does not provide privacy guarantees against network monitoring of the type or pattern of traffic it generates. That is to say, in some cases, the specifics of use may be able to remain private, but the use of this tool may be apparent to network observers.

## Zingo CLI
`zingo-cli` is a command line Zingo lightwalletd-proxy client. To use it, see "compiling from source" below. Releases are currently only provisional, we will update the README as releases come out.

## Privacy 
* While all the keys and transaction detection happens on the client, the server can learn what blocks contain your shielded transactions.
* The server also learns other metadata about you like your ip address etc...
* Also remember that t-addresses don't provide any privacy protection.

### Note Management
Zingo-CLI does automatic note and utxo management, which means it doesn't allow you to manually select which address to send outgoing transactions from. It follows these principles:
* Defaults to sending shielded transactions, even if you're sending to a transparent address
* Sapling funds need at least 5 confirmations before they can be spent
* Can select funds from multiple shielded addresses in the same transaction
* Will automatically shield your transparent funds at the first opportunity
    * When sending an outgoing transaction to a shielded address, Zingo-CLI can decide to use the transaction to additionally shield your transparent funds (i.e., send your transparent funds to your own shielded address in the same transaction)

## Compiling from source

#### Pre-requisites
* Rust v1.37 or higher.
    * Run `rustup update` to get the latest version of Rust if you already have it installed
* Rustfmt
    * Run `rustup component add rustfmt` to add rustfmt
* Build tools
    * Please install the build tools for your platform. On Ubuntu `sudo apt install build-essential gcc`

```
git clone https://github.com/zingolabs/zingolib.git
cd zingolib
cargo build --release
./target/release/zingo-cli
```

This will launch the interactive prompt. Type `help` to get a list of commands.

## Notes:
* If you want to run your own server, please see [zingo lightwalletd](https://github.com/zingolabs/lightwalletd), and then run `./zingo-cli --server http://127.0.0.1:9067`
* The log file is in `~/.zcash/zingo-wallet.debug.log`. Wallet is stored in `~/.zcash/zingo-wallet.dat`
* Currently, the default, hard-coded `lightwalletd` server is https://lwdv3.zecwallet.co:443/. To change this, you can modify line 25 of `lib/src/lightclient/lightclient_config.rs`

## Running in non-interactive mode:
You can also run `zingo-cli` in non-interactive mode by passing the command you want to run as an argument. For example, `zingo-cli addresses` will list all wallet addresses and exit. 
Run `zingo-cli help` to see a list of all commands. 

## Options
Here are some CLI arguments you can pass to `zingo-cli`. Please run `zingo-cli --help` for the full list. 

* `--server`: Connect to a custom zcash lightwalletd server. 
    * Example: `./zingo-cli --server 127.0.0.1:9067`
* `--seed`: Restore a wallet from a seed phrase. Note that this will fail if there is an existing wallet. Delete (or move) any existing wallet to restore from the 24-word seed phrase
    * Example: `./zingo-cli --seed "twenty four words seed phrase"`
 * `--recover`: Attempt to recover the seed phrase from a corrupted wallet
 * `--data-dir`: uses the specified path as data directory.
    * Example: `./zingo-cli --server 127.0.0.1:9067 --data-dir /Users/ZingoRocks/my-test-wallet` will use the provided directory to store `zingo-wallet.dat` and logs. If the provided directory does not exist, it will create it.

## Regtest
Experimental!
We have recently added support for `Network::Regtest` enum: https://github.com/zcash/librustzcash/blob/main/zcash_primitives/src/constants/regtest.rs
This has not been sufficiently tested, but now compiles.

This documentation to run regtest "manually" is a placeholder until flags are more functional.
We also aim to be able to select any network type with cli flags.

The CLI can work in regtest mode, by locally running a `zcashd` and `lightwalletd`.

For example:
Create the directories `~/.zcash/zd/` and `~/.zcash/ld`
`zcashd` works in regtest with the following invocation, run from your `zcash` directory (explicit path, substitute your own username):
`./src/zcashd --printtoconsole -conf=/home/username/.zcash/zd/zcash.conf --datadir=/home/username/.zcash/zd/`
and the following in a `zcash.conf` file in the specified directory:
```
regtest=1
nuparams=5ba81b19:1 # Overwinter
nuparams=76b809bb:1 # Sapling
nuparams=2bb40e60:1 # Blossom
nuparams=f5b9230b:1 # Heartwood
nuparams=e9ff75a6:1 # Canopy
nuparams=c2d6d0b4:1 # NU5

txindex=1
insightexplorer=1
experimentalfeatures=1
rpcuser=xxxxxx
rpcpassword=xxxxxx

rpcport=18232
rpcallowip=0.0.0.0
```

In another terminal instance, navigate to your `lightwalletd` directory, and run
`./lightwalletd --no-tls-very-insecure --zcash-conf-path ~/.zcash/zd/zcash.conf --data-dir ~/.zcash/zd/ --log-file ~/.zcash/ld/lwd-log --log-level 7`
`lightwalletd`'s terminal output will not have clear success, but the log file will show something like:
`{"app":"lightwalletd","level":"info","msg":"Got sapling height 1 block height 0 chain regtest ..."}`

Now you will need to add blocks to your regtest chain if you have not done so previously.
In still another terminal instance in the `zcash` directory, you can run `./src/zcash-cli -regtest generate 11` to generate 11 blocks.
Please note that by adding more than 100 blocks it is difficult or impossible to rewind the chain. The config means that after the first block all network upgrades should be in place.

Finally, from your `zingolib` directory, with a release build (`cargo build --release`), you can run:
`./target/release/zingo-cli --regtest --server=127.0.0.1:9067`
You should see a single line printed out saying `regtest detected and network set correctly!` and the interactive cli application should work with your regtest network!

Tested with `zcash` commit `1e6f46`, `lightwalletd` commit `db2795`, and `zingolib` commit `252abe` or better.

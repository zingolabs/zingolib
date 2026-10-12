## Zingolib
[![license](https://img.shields.io/github/license/zingolabs/zingolib)](LICENSE) [![coverage](https://img.shields.io/endpoint?url=https://zingolabs.org/zingolib/coverage/badge.json)](https://zingolabs.org/zingolib/coverage/)
This repo provides both a library for zingo-mobile, as well as an included cli application to interact with the Zcash blockchain through a chain indexer.

# Security Vulnerability Disclosure

If you believe you have discovered a security issue, please contact us at:

zingodisclosure@proton.me

## Zingo CLI
`zingo-cli` is a command line indexer client. Its own README,
[zingo-cli/README.md](zingo-cli/README.md), holds everything specific to it:
the one procedure that builds it, which bundles the `nym-proxy` binary beside
it, and how to launch it, take it online, choose a network (regtest included),
create or restore a wallet, and set its session options. Releases are currently
only provisional, we will update the README as releases come out.

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

## Architecture decision records

Architecture decision records live in
[zingo-adrs](https://github.com/zingolabs/zingo-adrs), and zingolib's own
records sit under `docs/adr/zingolib/` once the submodule is initialised. The
[zingo-adrs README](https://github.com/zingolabs/zingo-adrs#pointing-a-code-repository-at-zingo-adrs) explains how to read them, advance the pointer,
and propose a record.

## Testing
`run_workspace_tests.sh` script may be used as a helper to run all tests in one invocation.

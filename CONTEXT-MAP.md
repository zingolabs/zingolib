# Context map

This repository holds two bounded contexts, each with its own glossary.
A term's meaning is defined by its context's `CONTEXT.md`; when the same
word appears in both, each context's definition governs within that
context.

## Wallet-library domain

The domain of the shipped artifacts: the wallet, its keys, pools,
sync engine, send flow, and consumers (zingo-mobile, zingo-cli).

- Glossary: [`zingolib/CONTEXT.md`](zingolib/CONTEXT.md)

## Test infrastructure

The domain of the integration-test harness: scenarios, network combos,
chain caches, and observability instruments.

- Glossary: [`zingolib_testutils/CONTEXT.md`](zingolib_testutils/CONTEXT.md)

## Architecture decision records

Decisions live in [zingolabs/zingo-adrs](https://github.com/zingolabs/zingo-adrs),
which this repository holds as a submodule at `docs/adr/`. zingolib's records
sit under [`docs/adr/zingolib/`](docs/adr/zingolib/), and the org-scoped
records that bind every zingolabs repository sit at the top of `docs/adr/`.
Run `git submodule update --init docs/adr` to read them, and propose a record
in zingo-adrs, never here.

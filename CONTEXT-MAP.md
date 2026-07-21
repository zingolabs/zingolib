# Context map

This repository holds three bounded contexts, each with its own glossary.
A term's meaning is defined by its context's `CONTEXT.md`; when the same
word appears in both, each context's definition governs within that
context.

## Wallet-library domain

The domain of the shipped artifacts: the wallet, its keys, pools,
sync engine, send flow, and consumers (zingo-mobile, zingo-cli).

- Glossary: [`zingolib/CONTEXT.md`](zingolib/CONTEXT.md)

## View-model domain

The presentation domain: the editorial reductions consumers show to
users, derived from the wallet-library domain's canonical data. The
wallet-library domain never depends on it.

- Glossary: [`zingo-viewmodel/CONTEXT.md`](zingo-viewmodel/CONTEXT.md)

## Test infrastructure

The domain of the integration-test harness: scenarios, network combos,
chain caches, and observability instruments.

- Glossary: [`zingolib_testutils/CONTEXT.md`](zingolib_testutils/CONTEXT.md)

## Architecture decision records

System-wide decisions live in [`docs/adr/`](docs/adr/).

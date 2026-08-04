# Perspective domain

The presentation domain of the wallet stack: the editorial reductions
consumers (zingo-mobile, zingo-pc, zingo-cli) show to users, derived
from the wallet-library domain's canonical data. A reduction belongs
here whenever two reasonable consumers could legitimately want a
different answer; the wallet library keeps only Canonical Reductions.
The crate is zingo-perspective, singular by ruling: one house
perspective, N renderers.

## Language

**Editorial Reduction** — A derivation over canonical wallet data that
embodies a product opinion: what to show, what to hide, how to word it.
The complement of the wallet-library domain's Canonical Reduction.

**Funnel** — This crate's second role (ADR 0024 rule 7, amendment
2026-08-03): the single wallet dependency governed consumers declare.
Its modules re-export the wallet library's consumer surface
path-for-path, so a consumer repoints by renaming the dependency.
The funnel re-exports and projects; it never redefines.

**ValueTransfer** — A single directional movement of funds within a
transaction, grouped by recipient: received, sent, shielded, or
self-sent. The editorial per-recipient view of a transaction, derived
from the canonical TransactionSummary.
_Avoid_: payment, transfer record

**Self-Send** — A ValueTransfer whose recipient is the wallet's own
address. Editorially valued at zero, because no funds left the wallet;
the gross amounts remain visible in the canonical TransactionSummary.
Its kinds are basic, shield, memo-to-self, refund, and migration.

**Memo-to-Self** — The Self-Send kind produced when a sending
transaction also carries received memos, so the message is shown even
though no external payment exists.

**Migration Self-Send** — The Self-Send kind for an Orchard -> Ironwood
pool movement (the NU6.3 migration shape). Its classification predicate
wins over the memo check, pinned by the relocated mock-chain tests.

**Finsight** — The per-recipient rollups (total value, number of sends,
memo bytes, each keyed by recipient address) derived from
ValueTransfers for financial-insight displays.

# View-model domain

The presentation domain of the wallet stack: the editorial reductions
consumers (zingo-mobile, zingo-cli) show to users, derived from the
wallet-library domain's canonical data. A reduction belongs here
whenever two reasonable consumers could legitimately want a different
answer; the wallet library keeps only Canonical Reductions.

## Language

**Editorial Reduction** — A derivation over canonical wallet data that
embodies a product opinion: what to show, what to hide, how to word it.
The complement of the wallet-library domain's Canonical Reduction.

**ValueTransfer** — A single directional movement of funds within a
transaction, grouped by recipient: received, sent, shielded, or
self-sent. The editorial per-recipient view of a transaction, derived
from the canonical TransactionSummary.
_Avoid_: payment, transfer record

**Self-Send** — A ValueTransfer whose recipient is the wallet's own
address. Editorially valued at zero, because no funds left the wallet;
the gross amounts remain visible in the canonical TransactionSummary.

**Memo-to-Self** — The Self-Send kind produced when a sending
transaction also carries received memos, so the message is shown even
though no external payment exists.

**Text-only Memo Policy** — The editorial choice that only text memos
are rendered; empty, arbitrary-bytes, and future-format memos are not
shown. The canonical layer carries every memo losslessly typed.

**Finsight** — The per-recipient rollups (total value, number of sends,
memo bytes, each keyed by recipient address) derived from
ValueTransfers for financial-insight displays.

**Mixnet Status View** — The consumer-facing rendering of the
wallet-library domain's Mixnet Mode: the tri-state and, when ready, the
local proxy address, worded once for every consumer.

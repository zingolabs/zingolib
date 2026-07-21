# Editorial reductions live in zingo-viewmodel

zingolib's summary layer had grown two jobs at once: canonical,
key-aware classification of wallet transactions, and editorial choices
about how to display them — self-send value zeroing, a text-only memo
policy applied silently at seven construction sites, empty-memo
filtering, and per-recipient rollups. Consumers could neither opt out
of the opinions nor recover the data they discarded. We split the
layers: zingolib keeps only Canonical Reductions, and the editorial
layer moved to a new opt-in `zingo-viewmodel` crate.

The boundary rule: a reduction stays in zingolib only when **both**
hold — (a) *privilege*: computing it correctly needs wallet secrets,
protocol constants, or chain state the consumer doesn't cheaply have,
and (b) *no reasonable disagreement*: two reasonable consumers could
not legitimately want a different answer. Everything failing either
test is editorial. Under this rule `transaction_summaries`,
classification (`is_wallet_address`, transaction kinds), and balances
stay; the `ValueTransfer` derivation, memo display policy,
`messages_containing`, the `do_total_*` rollups, and the Mixnet Mode
status wording moved.

## Consequences

- Consumers migrate by adding the `zingo-viewmodel` dependency and one
  `use` of an extension trait (`LightWalletViewModelExt`,
  `LightClientViewModelExt`): the traits preserve the exact method
  names, signatures, and JSON shapes zingolib carried, and a golden
  harness (`zingo-viewmodel/tests/golden`) pins those outputs
  byte-for-byte against captures made from the pre-extraction code.
- zingolib never depends on zingo-viewmodel, so the editorial crate
  can never leak opinions back into the canonical layer; three
  lossless accessors (`pools_present`, `shielded_notes_by_pool`,
  `received_memos`) became `pub` so the derivation works from public
  canonical data like any consumer.
- Canonical memos are typed `zcash_protocol::memo::Memo` end to end;
  the raw summaries JSON renders a memo as a lossless kind-plus-content
  object, a visible change to that (non-mobile-facing) output, while
  the view-model applies the text-only policy consumers expect.
- The Mixnet Mode tri-state (ADR 0011) is worded once, in the
  nym-gated `MixnetStatusView`; zingo-cli renders it, and zingo-mobile
  receives the same view as JSON when it adopts mixnet UI.

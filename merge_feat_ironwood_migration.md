# Merge record: `feat/ironwood-migration` → `upgrade_to_ironwood_01`

This document records every non-mechanical decision made while resolving
the merge of PR #2428's tip (`40e9327c7`, `zingolabs/feat/ironwood-migration`,
which includes its own merge of `feat/ironwood`) into
`upgrade_to_ironwood_01`. The merge base is `e4e731358`; our side carries
113 commits past the base (chain caches, the observatory, the offline-twin
migration, test coalescence, the silent-spend fix), theirs carries 19 (the
ironwood dependency universe and the proof-of-concept migration flow).

The guiding rule: their side's base is older than most of our test-suite
restructuring, so where both sides touched test files, ours wins the
structure and any genuine deltas from their side are ported into the
surviving twin-siblings rather than dropped silently. Every such port is
itemized below.

## Merge decisions

### 1. Infrastructure pin: keep ours (`bump_to_NU6.3`), reject theirs (`84d9c97`)

Theirs pins `zcash_local_net`/`zingo_test_vectors` to infrastructure rev
`84d9c979` (the 0.6.0 lineage, which depends on zebra crates for witness
serving). Ours pins branch `bump_to_NU6.3` (0.7.0: front-proxy observers,
`from_parts`, launch-mine suppression under `chain_cache`) — the entire
chain-cache and observatory machinery exists only against our pin, and our
infrastructure tree depends on zero zcash\*/zebra\* crates (verified in the
lockfile), so the lrz source swap does not touch it. **Kept ours.**

### 2. Zebra `[patch.crates-io]` entries: deleted as unused

Their `zebra-chain`/`zebra-rpc`/`zebra-node-services` patches exist only
because *their* infrastructure pin names zebra crates. With decision 1,
nothing in our graph depends on any zebra crate, making the patches unused
(cargo warns and drops them from the lock). **Deleted the three entries
and their explanatory comment.** Whether infra should ever take zebra
crates is an open question for the infrastructure repo
(docs/testing/ironwood-regtest-upgrade-strategy.md, phase 2).

### 3. The librustzcash ironwood universe: adopted verbatim

The `[patch.crates-io]` source swap of the lrz family to
`zcash/librustzcash` rev `4d9a68dc`, orchard `0.15.0-pre.1`, the
`lightwallet-protocol` fork rev `9bdfdc77` with `rebuild-proto` (build
environments now need `protoc`), `.cargo/config.toml` enabling
`--cfg zcash_unstable="nu6.3"`, and the toolchain bump to 1.91 all
auto-merged and are **accepted as staged** — this is the phase-2 target
set the strategy doc already adopted.

### 4. Workspace dependency additions: union

Ours added `hmac`; theirs added `blake2b_simd`. Disjoint — **kept both.**
`zingo-netutils`/`zingo_common_components` converged identically on both
sides (0.4.0); the conflict was formatting only — **kept ours' single-line
form.**

### 5. `zingolib/src/lightclient.rs` module list: union

Ours added `#[cfg(test)] mod darkside;` and `#[cfg(test)] mod
mock_chain_tests;` (the offline-twin migration); theirs added `pub mod
migrate;` (the migration flow). No tension — **kept all three.**

### 6. Four test files (`concrete.rs`, `wallet.rs`, `chain_generics.rs`,
`fixtures.rs`): take ours, with an audited port of their deltas

All conflict hunks in these files follow one pattern: our side deleted or
restructured the regions (offline-twin migration, coalescence, `mod fast`
dissolution) while their side kept the base content with the mechanical
`ShieldedProtocol` → `ShieldedPool` rename. A scripted base-vs-theirs diff
of **every** hunk, with rename noise filtered, found exactly three genuine
deltas, all in `concrete.rs`:

- **Hunk 4** — `send_to_transparent_and_sapling_maintain_balance`: seven
  expected-`TransactionSummary` literals gain
  `ironwood_notes: vec![]` / `outgoing_ironwood_notes: vec![]`.
  **Ported into both twin-siblings**
  (`zingolib/src/lightclient/mock_chain_tests.rs` and
  `libtonode-tests/tests/unit_test_twins.rs`), seven pairs each.
- **Hunk 7** — the shield-underflow error match refactored from nested
  `match`/`if let` to `let-else` with a real panic message (same asserted
  values). **Ported into the live original** (`unit_test_twins.rs`); the
  offline twin was already rewritten as a flat single-pattern match that
  supersedes it — left as is.
- **Hunk 2** — `diversified_addresses_receive_funds_in_best_pool` (a test
  our side rewrote): its `PoolBalances` literal gains
  `total/confirmed/unconfirmed_ironwood_balance: Some(0)`. The staged
  `wallet/balance.rs` adds those fields to the struct itself, so every
  surviving constructor fails to compile until it carries them — this
  class of delta is compiler-enforced, and the legs are added at each
  surviving literal during the compile wave (entry to be completed with
  the list of touched files).

Everything else in the four files was rename-only on their side and moot
on deleted code. **Resolved take-ours.**

### 7. `pepper-sync/src/wallet/traits.rs` import: resolved to the merged body

The one three-way line: ours added `sync::set_transactions_failed_unchecked`
(the silent-spend fix's variant), theirs dropped `Orchard, Sapling` (their
domain generalization). The import is resolved to exactly what the merged
body references. Note the guarded `set_transactions_failed` (refuses to
fail `Confirmed` transactions — commit `b07a49673`) still exists and is
re-exported at the crate root, so their auto-merged call sites inherit the
guard by default. (Entry to be completed with the final import line.)

### 8. `Cargo.lock`: never hand-merged

Resolved by taking one side wholesale and letting cargo re-resolve against
the reconciled manifests; with decision 2 the zebra subtree drops out of
the lock. (Entry to be completed with the regeneration command used.)

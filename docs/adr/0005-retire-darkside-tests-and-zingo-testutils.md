# Retire the darkside-tests crate and the orphaned zingo-testutils crate

The `darkside-tests` crate and the `zingo-testutils` crate are removed
from the workspace. Darkside's reorg-semantics protections are preserved
by the mock-indexer `darkside` module in zingolib
(`src/lightclient/darkside.rs`); the wire-protocol vocabulary
("lightwalletd protocol") stays, because that names the protocol, not the
binary.

## Context

`darkside-tests` drove `darksidewalletd` (lightwalletd's adversarial
mode) to exercise how a wallet behaves when the chain reorganizes.
Every one of its nine tests had been `#[ignore]`d for months on two
darkside-mode bugs in lightwalletd itself: an invalid block-hash length
in served tree states, and a post-reorg `prev_hash` discontinuity. The
crate therefore protected nothing at runtime while holding the whole
legacy stack (lightwalletd, the `zcash_local_net` `legacy-stack` feature)
in the dependency graph for its sake, and while carrying a subtractive
pepper-sync feature (`darkside_test`) whose activation under
`cargo --workspace` removed `GetSubtreeRoots` calls from every crate in
the build by feature unification, the mechanism behind
zingolabs/zingolib#2447.

`zingo-testutils` was already a dead directory: not a workspace member,
depended on by nothing, retaining a lightwalletd service proto by pure
inertia. Its deletion costs no capability. The current test-utility crate
is the separately named `zingolib_testutils`, which is untouched, as is
the `test_lwd_zebrad` feature it defines.

## Considered Options

Preserving the darkside originals behind the `unit_test_twins` gate, as
the eight portable libtonode tests were preserved, was rejected. That
pattern earns its keep only for tests that still *run*: a gated original
is a live control group that can be re-executed to verify its twin. The
darkside nine cannot run at all, so gating them would archive
unbuildable code, dragging the `DarksideConnector`, lightwalletd, and
the poisoning feature forward, and demanding the dead code be maintained
through every future API migration, all to preserve what `git` already
preserves verbatim at `4d6a65236^`. This ledger is the durable record
instead.

Removing the now-orphaned `darkside_test` and `darkside_tests` cargo
features outright was deferred, not chosen. With `darkside-tests` gone,
nothing enables either feature in any workspace build, so their
subtractive `cfg` gates never activate and #2447's poison is already
cured. The features are retained *declared but unenabled* so their ~38
`cfg(feature = "darkside_test")` sites in pepper-sync stay recognized and
warning-free; a mechanical sweep of those gates is scoped follow-up work,
tracked in the feature comments.

## Consequences

- The darkside reorg protections run offline in seconds via the
  mock-indexer `darkside` module, in every default zingolib test pass,
  instead of never. Three tests there carry five of the nine originals'
  protections; the module doc records the four deliberately unported
  (two transaction-index variants awaiting an index surface on the
  summaries, and two proptest fuzzers).
- The workspace no longer builds or ships `darkside-tests` or
  `zingo-testutils`. `run_workspace_tests.sh` collapses to a single
  `cargo nextest run --workspace`.
- #2447 is resolved at its root: the subtractive feature can no longer be
  activated. When the offline `from_t_z_o` twin (ignored on #2447) is
  un-ignored, the live original temporarily restored to `concrete.rs`'s
  default suite returns behind the `unit_test_twins` gate.
- Eliding lightwalletd entirely from this repository now turns only on
  retiring the opt-in `test_lwd_zebrad` combo and confirming the Ironwood
  pool-migration broadcast path (which uses a generic gRPC client at a
  configurable URI) need not target lightwalletd. Neither is blocked by
  this change; both are separate decisions.

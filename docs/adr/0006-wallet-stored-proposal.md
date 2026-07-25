# The pending proposal lives in the wallet, and offline signing reads it

The `LightWallet` stores the pending proposal (`send_proposal`) between
proposing and Transmission, as it long has: the `propose_*` methods store
on success, and `send_stored_proposal` (the one-shot online send) or
`calculate_stored_proposal` (offline signing) consumes the slot. We
implemented and then withdrew a pure-core alternative in which shells (the
zingo-cli session, zingo-mobile's FFI bridge) held a `ZingoProposal` value
and the library stored nothing. The stored slot won because it minimizes
divergence from `dev` and leaves every consumer's two-call send protocol
untouched, zingo-mobile's `send` then `confirm` in particular. The slot is
process-lifetime state only: wallet reads reset it, and persisting
proposals across restarts remains an explicit non-goal. A proposal's
target height and note selection both decay, so reviving a stale proposal
correctly is re-proposing.

The Indexerless capability set fits the slot. Proposing is Indexerless and
stores. `calculate_stored_proposal` consumes the slot and signs with no
Indexer, leaving Calculated Transactions in the wallet. `transmit_calculated`
requires an Indexer and fails typed while the Calculated Transactions wait.
The send path's Offline gates sit before calculation, so a one-shot online
send never strands Calculated transactions it cannot transmit, and an
Indexerless `send_stored_proposal` fails before consuming the slot. Typed
errors remain a co-requisite (issue #2446): an Indexerless client makes
network-operation failure the routine path, so those failures must be
distinguishable by type. Proposal retargeting (see ADR 0008) resolves the
expiry of offline-calculated transactions (issue #2455): an Indexerless
`calculate_stored_proposal` lifts the stored target to the last height of
the wallet's consensus-branch epoch before building, so the Calculated
Transactions stay transmittable until the next scheduled network upgrade,
while a connected calculation keeps the ordinary expiry of forty blocks
(`DEFAULT_TX_EXPIRY_DELTA`) past the target.

## Considered Options

The withdrawn value-holding design made propose and calculate value-in,
value-out and moved all protocol state to the shells. It cost a
session-state thread through the CLI command dispatch and a breaking
change to zingo-mobile's `confirm()`, and it bought a referential
transparency we chose not to price above a minimal diff against `dev`.
Issue zingolabs/zingolib#2535 enumerates the withdrawn design's benefits,
so the trade can be repriced when a breaking-change window opens. We
rejected passing the proposal across the FFI as a serialized value for
the same churn plus a durability that was false as then measured: at the
time of the decision, every calculated transaction expired forty blocks
(`DEFAULT_TX_EXPIRY_DELTA`) past its target wherever the proposal was
held. The ADR 0008 retarget has since stretched the offline product's
life to its epoch's last height, so the durability leg of that rejection
is weaker than first recorded and the churn argument carries it. The
decay of note selection and anchors still makes re-proposing the correct
revival of a long-carried proposal.

# The pending proposal lives in the wallet; offline signing reads it

Between proposing and Transmission the pending proposal is stored in the
`LightWallet` (`send_proposal`), as it long has been: the `propose_*`
methods store on success, and `send_stored_proposal` (the one-shot online
send) or `calculate_stored_proposal` (offline signing) consumes the slot.
We implemented and then withdrew a pure-core alternative in which shells
(the zingo-cli session, zingo-mobile's FFI bridge) held a `ZingoProposal`
value and the library stored nothing: the stored slot won because it
minimizes divergence from `dev` and leaves every consumer's two-call send
protocol (zingo-mobile's `send` then `confirm` in particular) untouched.
The slot is process-lifetime state only: wallet reads reset it, and
persisting proposals across restarts remains an explicit non-goal, because
a proposal's target height and note selection both decay, so reviving a
stale proposal correctly is re-proposing.

The Indexerless capability set fits the slot. Proposing is Indexerless and
stores; `calculate_stored_proposal` consumes the slot and signs with no
Indexer, leaving Calculated Transactions in the wallet; and
`transmit_calculated` requires an Indexer, failing typed while the
Calculated Transactions wait. The send path's Offline gates sit before
calculation, so a one-shot online send never strands Calculated
transactions it cannot transmit, and an Indexerless `send_stored_proposal`
fails before consuming the slot. Typed errors remain a co-requisite
(issue #2446): an Indexerless client makes network-operation failure the
routine path, so those failures must be distinguishable by type. The
expiry of offline-calculated transactions (issue #2455) is resolved by
proposal retargeting (see ADR 0008).

## Considered Options

The withdrawn value-holding design made propose and calculate value-in,
value-out and moved all protocol state to the shells. It cost a
session-state thread through the CLI command dispatch and a breaking
change to zingo-mobile's `confirm()`, and bought a referential
transparency we chose not to price above a minimal diff against `dev`.
Passing the proposal across the FFI as a serialized value was rejected for
the same churn plus a false durability: transaction expiry (roughly forty
blocks) caps the useful lifetime of a carried proposal regardless of where
it is held.

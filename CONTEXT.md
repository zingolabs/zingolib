# Context

Glossary of terms as used in this repository.

## MinerPool

The pool a test validator mines block rewards to (transparent, Sapling, or
Orchard). An infrastructure-side concept owned by the `zingo-consensus` crate
(zingolabs/infrastructure); it describes validator configuration, not wallet
state.

Not to be confused with **PoolType**.

## PoolType

A Zcash value pool as seen from the wallet side (`zcash_protocol::PoolType`):
transparent, or shielded (Sapling/Orchard). Wallet code and test scenarios
reason in `PoolType`; only the boundary that configures a local test network
translates it into a **MinerPool**.

## Activation heights

The block heights at which Zcash network upgrades activate on a configured
test network. Two representations exist — the wallet-side one
(`zingo_common_components`) and the infrastructure-side one
(`zingo-consensus`) — and they are distinct types. Scenario APIs speak the
wallet-side representation; conversion happens only at the
`zcash_local_net` boundary (see `zingolib_testutils::scenarios`).

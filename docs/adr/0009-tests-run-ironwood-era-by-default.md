# Tests run Ironwood-era by default; Orchard-era behavior is opt-in

Every regtest and mock-chain suite in this repo runs on an NU6.3-configured
chain, so we decided that tests exercise the NU6.3/Ironwood era by default
and that their assertions must be Ironwood-aware: `check_client_balances!`
carries a required `i:` slot, so every call site states where its shielded
value actually lives rather than silently asserting only the legacy pools.

The era has two layers, and the suite must keep them distinct.

**The spec target.** ZIP 318 (zcash/zips#1317, "Turnstile and disabled
Orchard payments") disables ordinary payments into the old Orchard pool
once NU6.3 activates: wallets must send new payments to Orchard receivers
via the Ironwood pool, whose receiver in a Unified Address *is* the Orchard
receiver. Under the spec target, a V6 shielded payment or its change lands
in Ironwood, and the test fee oracle models the resulting V6 economics
(distinct orchard and ironwood bundles, each padded to its own two-action
floor).

**The adjudicated present.** The pinned zebrad/librustzcash stack does not
yet enforce the turnstile routing. Live server runs (2026-07-14) adjudicated
the open hypotheses: coinbase to a shielded miner pool lands as legacy
Orchard notes even for blocks past NU6.3 activation, and ordinary sends on
an NU6.3-active chain still produce Orchard outputs with V5-era ZIP 317
fees. Live-suite assertions therefore pin this present behavior — `i: 0`
with Orchard-era arithmetic — and each such assertion is a canary: when the
pinned stack begins enforcing the turnstile, these assertions fail loudly,
and the failure means "flip the pin to the spec-target expectation," not
"the wallet regressed."

Source pools stay literal in either layer: a test named `orchard_sends_*`
still funds the wallet with a real Orchard note and asserts it is spent,
because spending legacy Orchard notes is exactly what migration-era wallets
do (ZIP 318). Mixed-note wallets (legacy Orchard alongside Ironwood) are
legitimate fixtures, not accidents — behavior *relative to* Ironwood is a
primary test subject during the migration window.

Pre-Ironwood behavior appears in exactly two forms: tests that configure a
chain with NU6.3 unactivated (as `mine_to_orchard` does), on which the
builder derives a pre-V6 version from the branch id, and Orchard→Ironwood
migration tests, which inherently straddle both eras. There is no per-wallet
opt-out: the `allow_v6_transactions` setting was removed (wallet file
version 43) once the builder began deriving the transaction version from
the chain's branch id at target height, leaving the setting with no
behavioral reader. We rejected splitting the pool matrix by era (V5-pinned
copies of every Orchard-destination row) because the duplication was largely
redundant with the existing `ironwood_sends_*` rows.

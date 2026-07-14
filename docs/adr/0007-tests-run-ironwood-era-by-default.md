# Tests run Ironwood-era by default; Orchard-era behavior is opt-in

Once a chain configures NU6.3, the wallet proposes V6 transactions, and a
shielded payment to an Orchard receiver lands in the Ironwood pool — the
Ironwood receiver of a Unified Address *is* its Orchard receiver. Every
regtest and mock-chain suite in this repo runs on an NU6.3-configured chain,
so we decided that tests exercise V6/Ironwood-era behavior by default and
that their assertions must be Ironwood-aware: `check_client_balances!`
carries a required `i:` slot, destination pools in test names are
receiver-addressed (an "orchard" destination means the Orchard receiver, and
the output lands in Ironwood), and the test fee oracle models real V6
economics (distinct orchard and ironwood bundles, each padded to its own
two-action floor).

Source pools stay literal: a test named `orchard_sends_*` still funds the
wallet with a real Orchard note and asserts it is spent, because spending
legacy Orchard notes is exactly what migration-era wallets do (ZIP 318).
Mixed-note wallets (legacy Orchard alongside Ironwood) are legitimate
fixtures, not accidents — behavior *relative to* Ironwood is a primary test
subject during the migration window.

Pre-Ironwood behavior appears in exactly two forms: tests that pin
`WalletSettings::allow_v6_transactions: false`, which floors transaction
building at V5 and yields true Orchard outputs, and Orchard→Ironwood
migration tests, which inherently straddle both eras. We rejected splitting
the pool matrix by era (V5-pinned copies of every Orchard-destination row)
because the duplication was largely redundant with the existing
`ironwood_sends_*` rows; instead a single dedicated test covers the
`allow_v6_transactions: false` → V5 → Orchard-output path, keeping
zingo-cli's switch honest.

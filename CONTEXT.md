# zingolib

Zcash light-wallet library, CLI, and integration-test suites.

## Language

**Network combo**:
The Indexer+Validator pair a test scenario runs against, selected at compile
time in `zingolib_testutils`. The no-feature default is the Core stack
(zainod + zebrad); the only surviving alternative is lightwalletd + zebrad,
which dies with the Legacy stack.
_Avoid_: test stack, server pair

**Core stack** / **Legacy stack** / **Validator** / **Indexer**:
Defined in the infrastructure repo's `CONTEXT.md`
(github.com/zingolabs/infrastructure). This repo uses those terms with
identical meaning and does not redefine them. zcashd-backed network combos
were removed from this repo in July 2026; lightwalletd remains only for
darkside tests and the opt-in `test_lwd_zebrad` combo.

**Faucet**:
The test client whose spend capability receives the regtest Validator's
mining rewards, providing the funds most scenarios start from.
_Avoid_: miner client, funded client

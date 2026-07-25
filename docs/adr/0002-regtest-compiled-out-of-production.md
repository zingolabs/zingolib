# Regtest support is compiled out of production builds

`ChainType::Regtest` and the `ActivationHeights` vocabulary sit behind a default-off `regtest` cargo feature in zingolib and zingo-cli, mirrored by matching default-off features in zingo-mobile and zingo-pc. Shipped wallets must never contain regtest code paths. QA and development builds opt in with `--features regtest`, and the `testutils` feature implies `regtest`, so the test harnesses compile unchanged. The `ActivationHeights` type itself moves out of `zingo_common_components` (a crate being disintegrated) into `zingo-consensus`, a small leaf crate in the zingolabs/infrastructure workspace, which zingolib pulls only as an optional dependency of the `regtest` feature. A production build of zingolib therefore carries no activation-schedule dependency at all.

## Considered Options

We rejected re-homing the types directly into `zcash_local_net`: that crate is a process-launching test harness whose dependencies include the zebra stack, and zingolib's `ChainType::Regtest` variant participates in production wallet deserialization, so the harness would have ridden into every mobile and desktop build. We rejected duplicating the type per repo for the wallet family (the pattern zaino uses for its own config type) because every network upgrade would then touch several near-identical copies. Plain `cfg(test)` gating without a cargo feature was impossible: zingo-mobile and zingo-pc called the regtest constructors on shipped code paths, including address decoding, so only a feature boundary actually removes the code.

## Consequences

- Reading a wallet file whose chain tag is `2` (regtest) in a build without the feature fails with a descriptive error. The on-disk format is unchanged, so regtest wallets still open in regtest-enabled builds.
- Regtest addresses no longer decode in production builds of zingo-mobile and zingo-pc, which formerly tried `ChainType::Regtest` on every address parse.
- Cargo feature unification can silently re-enable the feature through any dependency that requests it, so release CI in zingolib, zingo-cli, zingo-mobile, and zingo-pc gains a shell-script tripwire that fails the build if the activation-heights crate appears in `cargo tree` for the release target.
- The helper formerly published as `activation_heights::for_test::all_height_one_nus` is absorbed into `ActivationHeights::default()`, documented as the default regtest activation schedule (every deployed upgrade active from block 1), because QA builds of shipped products call it outside of tests.

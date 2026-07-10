# Regtest activation heights enter zingo-cli only through `--activation-heights`

`zingo-cli --chain regtest` gains an `--activation-heights <PATH>` flag
naming a TOML file of network-upgrade activation heights, and this flag
is the only way to give the binary a regtest schedule other than the
compiled-in default. The schema is the one zcash-devtool already
consumes and the `zcash_local_net` harness already writes: one optional
`<upgrade> = <height>` line per upgrade, a missing key meaning the
upgrade never activates, an unknown key a hard error. The flag is
rejected outside `--chain regtest`. In harness use the file's contents
always originate from a query of the running Validator
(`WalletNetwork::from_validator`, infrastructure ADR 0003), so the
schedule the wallet interprets the chain under is the schedule the
chain was actually mined under.

The NU6.2 schema deliberately has no `nu6_3` key: `deny_unknown_fields`
turns a schedule from an NU6.3 chain into a loud error instead of a
silently truncated wallet view. The `nu6_3` key is added together with
the Ironwood wallet support. `nu7` is likewise excluded, matching the
devtool's release schema, which gates that key behind an unstable
build. The parser also validates that the schedule is prefix-contiguous
and non-decreasing before handing it to the `ActivationHeights`
builder, because the builder enforces the same invariant by panicking,
and malformed operator input must produce an error message rather than
a process abort.

## Considered Options

Per-upgrade CLI flags (`--nu5-activation 2 …`) were rejected because
the surface grows by one flag per future upgrade and the harness would
need zingo-cli-specific serialization instead of reusing the writer it
already has for the devtool. Extending the `--chain` argument with an
inline grammar (`regtest:nu5@2,…`) was rejected as a bespoke
mini-language no other component parses. Relying on the compiled-in
default schedule was rejected outright: it contradicts infrastructure
ADR 0003 (the Validator is the single source of truth for regtest
heights), and the default (every upgrade at height 1) does not match
any schedule the harness mines.

## Consequences

- The wallet binaries a `zcash_local_net` suite can drive (zcash-devtool
  and zingo-cli) share one activation-heights wire schema; harness-side
  wallet implementations differ only in the flag's plumbing, not in
  serialization.
- Every one-shot invocation against a regtest wallet must repeat the
  flag, because each invocation reconstructs `ChainType::Regtest` from
  scratch; the `zingo-cli-harness` crate therefore passes the file on
  every operation, not just at wallet creation.
- When ADR 0002 lands (regtest compiled out of production), the flag
  moves behind the same default-off `regtest` feature as the rest of
  the `--chain regtest` surface.

# PR #2470 review M2: split transport from rejection in the SOCKS5 errors

CLAIMED 2026-07-23 by the review-remediation session (the same session
that landed M1, ff64da547). Finding M2: `socks5_transmit.rs` maps every
post-connect `tonic::Status` to `Socks5TransmitError::Rejected`, whose
contract says "not a failover candidate" — but a Status includes
transport-shaped failures (DeadlineExceeded, Unavailable, Cancelled)
that the escalating fan-out exists to route around. The fix is a
hierarchical error taxonomy that separates the server's verdict from
the tunnel's health, mapped by `status.code()`, with pinned tests.

## File claims

- `zingo-netutils/src/socks5_transmit.rs` — the taxonomy, the mapping,
  and their tests.
- `zingo-netutils/src/error.rs` — only if the taxonomy lives there.
- `zingolib/src/lightclient/send.rs` — the `SocksTarget` consumption
  seam, if the typed split should reach `resilient_transmit`'s
  classification.
- `zingolib/src/lightclient/transmit.rs` — only if `classify_rejection`
  gains a typed entry point.

## Status

APPLIED (2026-07-23), uncommitted; commits are the user's to make. The
user sharpened the design mid-flight: the hierarchy must faithfully
report all data and sources, and the caller decides what to do with a
failure. What landed: every variant carries its phase's complete data —
`ProxyUnreachable`/`TunnelRefused` hold a typed source
(`ProxyDialFailure`/`TunnelFailure`) plus `elapsed: Duration`;
`TunnelTransport` keeps the rendered chain and the structured
`tonic::transport::Error` when one exists; a new `Rpc` variant carries
the whole `tonic::Status` as `#[source]`; `Rejected` now wraps a typed
`SendRejection { code, message }` (the shared `parse_send_response` was
typed accordingly, with the clearnet `GrpcIndexer` path keeping its
historical bare-message text). Policy lives in one method,
`is_failover_candidate()`: `Rejected` never, `Rpc` by a documented
status-code disposition whose asymmetry leans transport (a false
transport reading costs one redundant arm, duplicate-equals-success;
a false verdict reading suppresses failover), everything else yes.
Wallet-side consumption is unchanged (strings still flow into
`resilient_transmit`); the typed reading awaits the per-arm
retry-tuning follow-up.

Verified: netutils standalone fmt/clippy clean and nextest green (27
default, 35 with socks5-transmit, including 5 new pinning tests);
main-workspace clippy clean (default and nym,nym-diary, all targets);
42 zingolib nym/transmit tests green.

# PR #2470 review M4 (superseded scope): the price fetch is mixnet-only

CLAIMED 2026-07-23 by the review-remediation session (M1 ff64da547,
M2 6b9d002de, M3 0936a66d2). The original M4 asked only that reqwest's
`socks` feature stop being unconditional in zingo-price; the user
ratified the stronger rule mid-walk: the price fetch is DISALLOWED over
clearnet in every configuration — no per-session opt-out, unlike send
and migration, because the fetch contacts a third-party price API whose
value is cosmetic and whose clearnet contact leaks wallet-alive timing
and the client IP to a non-Zcash party. Consequently the whole fetch
path (and reqwest with it) gates behind the nym arc, and the default
dependency graph shrinks below its pre-PR shape.

## File claims

- `zingo-price/Cargo.toml`, `zingo-price/src/lib.rs` — optional reqwest
  behind a `socks5-fetch` feature; the fetch functions take a required
  SOCKS5 address; types and history storage stay unconditional.
- `zingolib/Cargo.toml` — `nym` forwards `zingo-price/socks5-fetch`.
- `zingolib/src/wallet.rs` — `update_current_price` gated on `nym`,
  required proxy address.
- `zingolib/src/lightclient.rs` — the route policy: mixnet or typed
  refusal, never clearnet.
- `zingolib/src/lightclient/error.rs` — the typed refusals.
- `zingo-cli/src/commands.rs` — the currentprice help made accurate.
- `docs/adr/0011-nym-mixnet-transmission.md` — amendment: the price
  fetch loses its clearnet tier.

## Status

APPLIED (2026-07-23), uncommitted; commits are the user's to make. The
single switch is zingolib's `nym` feature forwarding
`zingo-price/socks5-fetch` — the only configuration in which any fetch
code exists. zingo-price's five network-side deps (reqwest, rustls,
serde, serde_json, rust_decimal) are optional behind that feature; the
fetch functions take a REQUIRED SOCKS5 address, so even an enabled
build cannot express a clearnet fetch; the price types and wallet-file
serialization stay unconditional for file portability.
`LightClient::update_current_price` refuses with typed errors:
`PriceFetchRequiresMixnet` when Mixnet Mode is toggled off (no opt-out
tier, unlike send) and `PriceFetchUnsupported` in non-nym builds;
bootstrapping/died fail closed via `MixnetNotReady`. The currentprice
CLI help now states the rule; ADR 0011 gained the "price fetch loses
its clearnet tier" amendment.

Verified: zingo-price checks alone in both configurations; clippy clean
on default and nym,nym-diary (zingolib + zingo-cli, all targets);
`cargo tree` shows zero reqwest/tokio-socks edges in the default
zingo-cli graph (both present with nym); 42 gated zingolib tests and
104 zingo-cli tests green.

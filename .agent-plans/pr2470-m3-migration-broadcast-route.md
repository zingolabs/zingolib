# PR #2470 review M3: migration broadcasts obey Mixnet Mode

CLAIMED 2026-07-23 by the review-remediation session (M1 ff64da547,
M2 6b9d002de). Ratified in-session, refined mid-flight: with the `nym`
feature compiled in and Mixnet Mode engaged, migration parts travel
ONLY over the mixnet and NEVER over clearnet; the mixnet target must
never share the synchronization endpoint's host; document all of it.
Clearnet remains only for the deliberate per-session opt-out and for
builds without the feature, where behavior is unchanged.

## File claims

- `zingolib/src/lightclient/migrate/broadcast_route.rs` (new) — the
  routed client, the mixnet client, the pure candidate selection, tests.
- `zingolib/src/lightclient/migrate/broadcast_grpc.rs` — timeout
  visibility only.
- `zingolib/src/lightclient/migrate.rs` — the route-aware
  `migration_broadcast_client` and module docs.
- `zingolib/src/lightclient/error.rs` — the two typed refusals.
- `zingolib/src/config.rs` — the `migration_broadcast_uri` doc.
- `docs/adr/0011-nym-mixnet-transmission.md` — the 2026-07-23 amendment.
- `.github/workflows/ci-pr.yaml` — the broadcast-route test filter.

## Status

APPLIED (2026-07-23), uncommitted; commits are the user's to make.
`migration_broadcast_client` resolves the Mixnet Mode route first: Ready
→ `MixnetBroadcastClient` (one random Broadcast Indexer per submission —
Witness Rotation for migration parts — from `eligible_candidates`, which
refuses a configured target on the sync host and excludes that host from
the pool, typed errors `MigrationBroadcastTargetIsSyncEndpoint` /
`NoEligibleBroadcastIndexer`); Bootstrapping/Died → fail-closed
`MixnetNotReady` before any submission; Off or non-nym build → the
historical clearnet path unchanged. All four broadcast call sites
(scheduled, catch-up, auto, immediate) route through this one
constructor. M2's `is_failover_candidate` maps mixnet failures onto
`BroadcastError`'s Transport/Rejected contract.

Verified: clippy clean (default and nym,nym-diary, zingolib+zingo-cli,
all targets); 33 nym-side tests green including the 5 new
broadcast_route tests; 83 default-build migration tests green;
libtonode chain tests compile untouched.

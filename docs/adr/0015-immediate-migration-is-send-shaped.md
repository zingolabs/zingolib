# Immediate migration (Drain) is exposed as a send-shaped call

`LightClient::quick_immediate_migration(account, resume_sync)` is the entry
point for the immediate Orchard→Ironwood Drain. It mirrors `quick_send`: it
pauses sync internally, migrates against the wallet's current state without
synchronizing, takes no `SyncPauseGuard`, and restores the prior sync mode on
return unless `resume_sync` is `false`. The existing `migrate_immediately`
(self-syncing) and `migrate_immediately_presynced` (guard parameter) remain as
the Rust-internal forms; `quick_immediate_migration` is a thin wrapper over the
latter, and is what the CLI's `drain now` drives.

## Why

Both pre-existing immediate-migration entry points are uncallable from
zingo-mobile, the primary consumer of the immediate path:

- `migrate_immediately` calls `sync_and_await`, which collides with
  mobile's continuous background sync (`SyncModeError::SyncAlreadyRunning`).
- `migrate_immediately_presynced` takes `sync: &SyncPauseGuard`, an RAII
  borrow that cannot cross the UniFFI boundary.

`quick_immediate_migration` fills that gap in the idiom the codebase already
uses for sends: `quick_send` and `quick_shield` also pause internally and do not
self-sync. An immediate migration is send-shaped anyway — it is *transmitted*
over the ordinary send connection, not *broadcast* over the decoupled Migration
Broadcast Endpoint that the private path's Parts use.

## Trade-off

`migrate_immediately_presynced` makes "planning under a running sync"
unrepresentable at compile time by demanding the guard as a parameter (there is a
`compile_fail` doctest, and `zingolib/CONTEXT.md` frames this as a virtue).
`quick_immediate_migration` gives up that compile-time proof for the same
*runtime* guarantee `quick_send` relies on: it constructs the guard itself,
before the plan read, so the pause is held across plan and build regardless. The
guarded form stays available for Rust callers who want the stronger contract.

## Considered and rejected

- **A `target_pool`, an origin+destination pair, or renaming to `quick_migrate`.**
  Migration is a ZIP 318 consensus event that retires exactly one pool pair
  (Orchard→Ironwood), not a generic movement of funds between pools; "migrate"
  already names the private path (`migrate_to_ironwood`). Generic cross-pool
  consolidation is a `send_all` to one's own address — a separate concern that
  needs no migration machinery. See the **Migration** and **Drain** entries in
  `zingolib/CONTEXT.md`.

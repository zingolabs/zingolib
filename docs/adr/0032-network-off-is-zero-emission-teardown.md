# `network off` is zero-emission teardown, and `--offline` suppresses the network surface

Status: accepted (2026-08-05)

The `network` command's `on` and `off` were asymmetric: `on` was a
Connectivity Consent act that took an offline session online (ADR 0026),
while `off` merely disabled the mixnet and kept transmitting over
clearnet — a privacy downgrade that stayed in ONLINE MODE. Meanwhile
`--offline`, whose glossary contract is zero network traffic for the
life of the session, could be lifted mid-session by `network on`,
contradicting that contract. We decided to make the pair symmetric and
the flag absolute.

## Decision

`network off` disconnects every network capability of the session, as a
security contract: it kills the nym-proxy child, drops the Indexer
connection, clears the Migration Broadcast Endpoint, and aborts every
background emitter (mempool observation, in-flight sync), returning only
when teardown is complete. After it returns, no network-visible
information is emitted. The act is session-scoped: it never touches a
stored standing Connectivity Consent (`--forget-online` remains the one
erasure act), and its report says so when a standing consent exists,
because the next launch will attach again.

After `network off`, the session holds the same suppressed surface as an
unconsented first boot: network-requiring commands disappear from `help`
and refuse if typed, while the `network` family stays offered so
`network on` can re-consent. `help` reflects the live posture, and every
refusal names the live remedy — `network on` in a session that can
re-consent, relaunch-without-`--offline` in a deliberate `--offline`
session.

`--offline` is unliftable. A deliberate `--offline` session offers no
command that requires network access — including the whole `network`
family, whose subject (the session's network posture) is fixed — and
prints a launch notice saying so and naming the only exit: relaunch
without `--offline`. This narrows ADR 0026's consent-act clause:
`network on` remains an in-session Connectivity Consent act for
unconsented sessions only. Suppression is command-granular except for
`migration`, whose `plan`, `status`, and `windows` subcommands read
stored wallet state and stay available (the glossary promises Offline
mode every Indexerless capability); its syncing and broadcasting
subcommands refuse. Cached-read commands (`height`, `servers`) stay
offered and render as "Last Known" reports: they state their vintage
from stored data only — the mined time of the Last Known block where
available — and never probe.

The clearnet-transmit act is retired from zingo-cli. No CLI command
selects clearnet for Transmission or price-fetch, enforcing the
Sync-Only Clearnet policy at this consumer: clearnet serves only sync.
The library's switched-off Mixnet Mode state remains, reachable by other
consumers until they converge on the same policy; zingo-cli simply no
longer offers an act that reaches it.

## Considered options

Keeping `network off` as the mixnet toggle and adding a separate
teardown verb was rejected: two off-ish verbs invite the exact confusion
that made "off" dangerous (a user cutting the network must never land in
clearnet transmit). Renaming the downgrade to `network clearnet` was
rejected because the Sync-Only Clearnet policy says that transmit path
should not exist. Suppressing network-requiring commands in unconsented
first-boot sessions too was rejected: those sessions keep `network on`
as their in-session path online.

## Consequences

`help` output becomes a function of the live session posture, not of the
launch snapshot. The refusal strings and the launch notice are minted
CLI vocabulary and get pinning tests. A mixnet-capable online session
whose mixnet is not ready fails closed for Transmission and price-fetch
with no clearnet escape hatch; the remedies are `network status` and
waiting out the bootstrap.

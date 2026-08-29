# 29. A maintained mixnet indexer pool replaces server selection

Date: 2026-08-04

Status: superseded 2026-08-08 — the Destination Pools and the
transmissions-are-probes Health economy (glossary; ADRs 0038 through
0040) replace the maintained-connection model. The retirement of
server selection as a concept survives in ADR 0034's sweep.

## Context

ADR 0027 obsoleted the clearnet census ranking and left its replacement
open: "Server selection must be rebuilt without a clearnet census
probe." A 2026-08-04 audit established how far the old flow had decayed.
The ranking list (`MOST_UP_INDEXER_URIS`) held twelve URIs snapshotted
from the hosh.zec.rocks leaderboard on 2026-03-26; a live `GetLightdInfo`
sweep found all twelve dead, so the flow would have sprayed the user's
IP at whoever now holds those lapsed domains and then fallen back to the
default anyway. The same sweep, extended to every endpoint named by the
hosh monitor and by the ZODL, Zkool, and Vizor wallets, found ten
operators alive; the census now records their endpoints, and its
`operator` field is retired in favor of derivation from each endpoint
host's registrable domain (one domain is one administrative authority —
the sound direction of the operator inference; distinct domains prove
nothing, as two live endpoints serving a byte-identical custom
lightwalletd build illustrate).

Two transport facts bound the rebuild. First, the packet model: the
mixnet's same-size, randomly timed Sphinx packets exist between the
client and its exit gateway only. The exit dials the indexer over
ordinary clearnet TCP, and the wallet's TLS rides end to end inside it,
so an indexer sees content-protected but ordinarily shaped traffic from
an exit's IP. IP concealment survives the exit hop; traffic shape does
not. Second, the exits themselves: the current exit policy reaches port
443 only, which excludes some census endpoints from every mixnet-routed
operation; one proxy instance binds exactly one exit, won by a hedged
race over the directory; and the directory offered 817 exit-capable
nodes on the audit date, so exit scarcity is not a constraint —
per-instance resource cost is.

Latency ranking, the old flow's organizing idea, is meaningless through
a mixnet: measured latency is dominated by Sphinx routing, not by the
server. And because contacting an indexer over the mixnet reveals no
client IP, the census-wide fan-out that was the old flow's privacy
defect becomes free.

## Decision

Server selection is retired as a concept. In its place the wallet keeps
a Maintained Indexer Pool: every census endpoint eligible for mixnet
transport (port 443, non-obsolete) is contacted over the mixnet, and
every successful connection is kept for the lifetime of the client.
"Kept" means supervised, not unbroken: connections die to server
restarts, idle-timeout load balancers, and exit churn, and a
per-endpoint state machine re-establishes them with bounded backoff.
There is no ranking and no winner; the pool's membership is defined by
liveness alone.

The pool obeys the budgeted exit-diversity invariant, ratified
2026-08-04: within a per-platform budget of standing transports, no
exit carries connections to more than one indexer operator. At full
width — one exit per endpoint, seventeen today — colluding indexers
cannot link the wallet's operator set through a shared exit address; a
platform that cannot afford full width (mobile, where each transport is
a bootstrap wait and a standing cover-traffic drain) partitions
endpoints across its smaller budget, never crossing operators within an
exit, and accepts the residual linkage inside each partition. Exit
assignment samples uniformly without replacement from the directory's
exit providers, drawing from distinct nym-node runners as far as the
directory's metadata allows, so exit-side diversity is not quietly
reconstituted into a single observer.

Boot-time discovery gives every candidate endpoint its own exit,
uniformly sampled without replacement from the directory's exit
providers. The census is small (seventeen eligible endpoints today) and
the exit population is not (817 on the audit date), so per-endpoint
uniqueness is affordable, and it removes the shared-exit boot signature
entirely: no exit observes more than one probe, and no operator learns
anything at boot beyond its own endpoint having been probed. A probe
that succeeds keeps its transport — the discovery dial is the pool
connection's first dial, and the exit that carried it carries the
maintained connection thereafter, so nothing is torn down and
re-established and no hand-off ever links two exits to one endpoint. A
probe that fails tears its transport down. Under per-endpoint exits the
diversity invariant holds by construction, an exit carrying one
endpoint carries at most one operator, and full width becomes one
transport per live endpoint; a platform that cannot afford full width
partitions endpoints across its smaller budget, still never crossing
operators within an exit.

The pool joins the ratified transport taxonomy as its own category.
Send transports rotate per Transmission (Exit Rotation's two-ready
pool); each Sync Session gets a dedicated fresh tunnel; discovery
transports graduate into pool transports on success and are torn down
on failure; pool transports stand for the client lifetime. No category
shares an exit with another.

The sync attach — bulk synchronization's single clearnet connection,
the one moment the wallet shows an indexer its real IP (ADR 0027) — is
aimed by a rule ratified 2026-08-05. The wallet draws uniformly at
random over the pool's live operators: one ticket per operator, not
per endpoint, because IP exposure accrues to the operating party, and
endpoint-uniform sampling would hand the largest fleet most of the
tickets. Any live endpoint of the drawn operator serves. The draw is
sticky for the Sync Session and redrawn at the next, so exactly one
operator sees the wallet's IP and sync cadence per session; an
explicit user pin overrides the draw entirely. The randomness
dissolves the ecosystem's default-server monoculture — every surveyed
wallet today deterministically aims at the same operator — and, with
ADR 0022, the draw also fixes which operator sits out witness duty for
the session.

## Consequences

The `server_select.rs` latency race is deleted rather than repaired,
and its tokio-runtime-nesting crash (the panic that prompted ADR 0027's
audit) goes with it. The proxy surface grows a caller-supplied exit
choice — today the hedged race picks the exit and exposes none — which
is parameter plumbing over the existing candidate-list machinery, not a
redesign.

Bulk synchronization below the Mixnet Sync Window remains the one
clearnet operation (ADR 0027), to exactly one indexer, aimed by the
sync-attach rule above. What `network on` reports while transports
bootstrap and the pool fills is the rewrite's remaining open question.
Whether an operator pair whose distinctness is unattested (two live
endpoints serve a byte-identical custom lightwalletd build) should
share one ticket in the sync draw is tracked with the census's
attestation work.

The mixnet's per-client throughput bounds what the pool may carry:
control traffic, broadcast fan-out, witness duties (ADR 0022 draws its
disjoint witness from the pool for free), and price — not bulk sync.
Census growth changes the budget arithmetic: full width today is
seventeen transports; a censused new endpoint raises it, and platforms
re-derive their partitions rather than hard-coding counts.

This decision implements the server-selection consequence of ADR 0027,
amends ADR 0026's launch-order clause (the indexer is no longer resolved
before the mixnet comes up — the mixnet is now the instrument of
resolution), and extends the exit-disjointness doctrine of Exit Rotation
and Sync Session to a third transport category.

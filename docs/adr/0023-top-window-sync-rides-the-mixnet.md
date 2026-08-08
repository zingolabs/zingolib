# Top-window sync rides the mixnet

Status: accepted (ratified 2026-07-28; re-founded 2026-08-06);
implementation deferred — all sync currently rides clearnet, and this
record is the target routing, not the shipped behavior (2026-08-06)

Compact-block sync was the last wallet surface that always rode
clearnet, and its top-of-chain segment is where timing linkage lives:
an indexer that serves a wallet's recent blocks can correlate them with
that wallet's transmissions. We decided that the top of the chain
always syncs over the Nym mixnet, and on 2026-08-06 we re-founded the
decision on one per-request axiom with two request classes.

## Decision

The mixnet carries every sync request that names the wallet or touches
the Mixnet Sync Window; the session route carries only deep,
wallet-neutral chain data.

Wallet-naming requests — full-transaction fetches by txid,
transparent-address queries, utxo metadata, and the mempool stream —
ride the mixnet at any height. Their sensitive subject is the wallet
itself, their payloads are small, and a pure height rule would hand
the indexer exactly the wallet-interest linkage the window exists to
prevent (a txid's height is not even knowable before the fetch).

Height-neutral structural requests — compact-block and nullifier
ranges, subtree roots, frontiers, tip queries — route by height: above
(tip − window size) they ride the mixnet, below it they ride the
session's existing route (the user's system-level NymVPN when one
runs, clearnet otherwise; the wallet never embeds a dVPN).

Two statements that read like extra rules are corollaries: a catch-up
that fits inside the window touches no clearnet, and the tip query is
itself windowed, so a mixnet-capable session's first contact is always
the mixnet.

Mixnet-bound requests fail closed. While the mixnet bootstraps they
wait; if it dies they refuse typed; they never fall back to clearnet.
Below-window structural requests, already consented to the session
route, proceed in parallel with bootstrap, so a deep catch-up overlaps
bootstrap time with useful work and the privacy boundary holds exactly
at the height rule.

The window's size is the Five-Minute Calibration: the block count
empirically downloadable over the mixnet in five minutes, the mobile
background-sync slot. The project measures it and ratifies the result
as a named constant per release — currently 30,000 blocks, about
twenty-six days at the 75-second target spacing. It is never measured
per device or per session.

## Considered options

A per-session rule ("decide once at sync start whether clearnet is
permitted") was rejected: a tip advance mid-sync strands the decision,
and the measuring tip query needs its own transport ruling anyway. A
runtime-adaptive window sized to each device's throughput was
rejected: it would give slow networks more clearnet exposure and make
the boundary height a fingerprint of the user's bandwidth — the
calibration bounds the typical catch-up's duration, never the privacy
guarantee. Gating all sync on mixnet readiness was rejected: it
serializes deep catch-ups behind bootstrap for no privacy gain on
blocks the rule already assigns to the session route. A pure height
rule without the wallet-naming class was rejected: an old txid fetch
over clearnet reveals wallet interest regardless of height.

## Consequences

Sync must route per request, not per session, so the sync engine's
transport choice becomes a function of request class and subject
height. The sync-over-socks5 gap (zingolib#2591) becomes the
implementation prerequisite. Clearnet's remaining role shrinks to
deep, wallet-neutral structural data, aligning with the Sync-Only
Clearnet policy and the Maintained Indexer Pool (ADR 0029): most
wallets, most days, emit nothing over clearnet at all.

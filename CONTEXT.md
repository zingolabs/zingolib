# zingolib

A Rust Zcash light-wallet library. The vocabulary below covers the
migration domain (ZIP 318, Orchard → Ironwood), which follows the ZIP's
language with gaps filled from the Shielded Labs migration-security
recommendations, and the session's network posture.

## Language

### Network vocabulary

**Nym / mixnet (the seam rule)** (ratified 2026-08-11):
The local SOCKS5 seam divides the vocabulary. Below the seam, "Nym"
names the vendor stack: nym-sdk, the Nym directory and gateways, the
`nym-proxy` binary, the `nym` cargo feature that compiles that stack
in, and the `network` CLI command where a user names the network they
trust. Above the seam, "mixnet" names the wallet's transport domain:
`zingolib::mixnet`, Mixnet Mode, the slot, the route, the status
channel, the probes, and the consent semantics.
_Avoid_: "nym" for the wallet-side domain; "mixnet" for the vendor stack

**Transmit policy** (ratified 2026-09-11):
The client's per-session choice of where a transaction travels,
`Mixnet` or `Clearnet`, held beside the transport slot and never in it.
Read by the send route alone: the price fetch and the probe are
mixnet-only and never consult it.
_Avoid_: "switched off" for the clearnet choice; that names a transport
state, which both routes read as a missing transport

**Mobile platform** (ratified 2026-08-11):
The embedding application layer on a subprocess-forbidding OS — the
mobile app hosting the proxy shim — which implements `ProxyHost` below
the seam and hands the wallet a ready SOCKS5 endpoint.
_Avoid_: bare "platform" for this sense; the bare word stays only for
the generic OS sense and for the desktop-or-mobile provisioning axis
(ADR 0041's platform-typed session)

**Exit-Proven** (ratified 2026-08-13):
The one validation an Exit Node carries: a completed round trip
through it — it answered the Sentinel (`proves_the_exit`), or it
carried a task. The only rung; readiness proves nothing about the
exit.
_Avoid_: "Exit-Bound" as a validation rung (deleted); "healthy" for
this fact

**EpochProven** (ratified 2026-08-14):
The Exit-Proven fact bounded by the epoch it survives: the
`ExitNodeHealthVerdict` a round trip earns, trusted until one Nym
epoch after the observation instant and stale after.
_Avoid_: bare "Proven" (the pre-epoch name, upgraded)

**PreviouslyProvenThisEpoch** (ratified 2026-08-14):
The Mixnet Mode of a Standing Client up on stale proof: born trusting
an EpochProven observation an earlier client earned, unconfirmed by
any round trip of its own; routes exactly as Ready, promoted by the
first confirmed round trip, demoted through ProofAcquisition.

**ProofAcquisition** (ratified 2026-08-14):
One adjudication of the Standing Client's exit: an arbiter Sentinel
exchange dialed into its tunnel, promotion on an answer, and on
silence a conviction followed by the two-layer failover; fired by
suspicion (an exit-implicating failure) or by the proof deadline
lapsing, whichever comes first.

**Proven Client** (ratified 2026-08-13):
A client whose exit is Exit-Proven — by its own birth probe, or by
drawing an exit whose fresh proof is trusted.
_Avoid_: "ShownHealthy", "ProvenHealthy"

**Standing Client** (ratified 2026-08-14):
The session's one long-lived Proven Client — the transport every
operation but the price fetch multiplexes over, born at go-online and
holding its bound exit's lease for its life.
_Avoid_: "session tunnel", "slot tunnel" for the client itself; "the
tunnel" names only its local SOCKS5 face

**Observation** (ratified 2026-08-13; amended 2026-08-14):
One exit's most recent verdict — EpochProven or Failed
(`ExitNodeHealthVerdict`) — with its timestamp; a later verdict for
the same exit supersedes it, and both lapse after one Nym epoch, so a
convicted node stands trial again once the topology that convicted it
has rotated away.

**NodeHealthIndex** (ratified 2026-08-13):
The session's memory of Observations, one per Exit Node, remembered
only through the Exit Pool's own draws and consulted to order
sampling: fresh-Proven first, then unknown, Failed only at exhaustion.

**Indexer classification** (ruled 2026-09-14, pending review):
Every indexer has a role (sync, broadcast, or both), a trust (whether
its operator may link the wallet to its transactions), and a location
(local, on the wallet's machine or network, or remote). The consumer
states role and trust; location comes from the URI. A local indexer is
trusted by default and a remote one takes the network's remote default.
_Avoid_: "relay origin" for location.

**Destination Server set** (ruled 2026-09-11, amended 2026-09-14, pending review):
The indexers one session may broadcast to: the ones the consumer
classified and the indexer registry for the wallet's chain, drawn by one
rule on every transport. Over the mixnet it excludes an untrusted sync
indexer's operator; over clearnet it never draws the registry; a trusted
candidate is drawn alone. Derived at session open, never serialized.
Distinct from the Exit Pool, which holds mixnet exits.
_Avoid_: "Destination pool", "curated Destination list".

### Command classes

**Readiness budget**:
The bounded time a transmitting command waits for a bootstrapping mixnet
to become ready before the typed refusal stands.

### Migration paths

**Immediate migration**:
The non-private ZIP 318 option ("migrate immediately"): every spendable
Orchard note swept into one Ironwood output per transaction, real amounts
visible on-chain, transmitted at once.
_Avoid_: drain

**Scheduled migration**:
The private ZIP 318 flow: note preparation into funding notes, then
transfers broadcast across windows. Two commit points: the plan, then the
schedule.

**Note preparation** (ratified 2026-09-17):
Phase 1 of the scheduled flow: Orchard self-sends that divide or combine
notes into funding notes, sized exactly denomination + transfer fee. The
ZIP 318 term.
_Avoid_: note splitting (the earlier local term; preparation also combines)

**Commit** (ratified 2026-09-17):
Recording the user's consent to a plan (the first commit point) or to a
proposed schedule (the second). ZIP 318's "the schedule is committed".
_Avoid_: start, confirm

**Consolidation**:
Merging fragmented notes into fewer notes without crossing pools. Within
note splitting, the round that merges fragments before sizing.
_Avoid_: reduction

### Amounts

**Denomination**:
A canonical migration amount, {1, 2, 5} × 10^k ZEC. What the Shielded Labs
document calls amount "buckets". Never use bucket for amounts here.

**Transfer** (ratified 2026-09-17):
One scheduled pool-crossing transaction carrying exactly one denomination.
ZIP 318's "migration transaction", also its "scheduled transfer".
_Avoid_: part (ZIP 318's parenthetical synonym, retired here)

**Funding note** (ratified 2026-09-17):
An Orchard note sized exactly denomination + transfer fee, so it funds one
transfer as it is. The ZIP 318 term.
_Avoid_: part-ready note

**Reserve / release** (ratified 2026-09-17):
A reserved note is never selected by an ordinary send. Committing a plan
reserves every pre-Ironwood Orchard note of the account; committing the
schedule narrows the reservation to the funding notes of the pending
transfers; releasing a transfer or cancelling frees them. Derived from the
migration record, never stored. Our term: ZIP 318 has none and assumes the
user may spend outside the migration.
_Avoid_: lock (collides with the wallet lock), mark, soft reservation

**Residual**:
Value the migration abandons: notes at or below the sweep minimum, plus
balance below the smallest denomination (`MAX_RESIDUAL_VALUE`).
_Avoid_: stranded, dust floor

**Sweep minimum**:
The selection floor: a note worth at most this is never selected, an output
worth at most this is never created.

### Scheduling

**Bucket**:
A time window of M consecutive blocks (ZIP 318 sense). Never an amount.

**Boundary**:
The block height that opens a bucket (height ≡ 0 mod M), also the anchor
height of the bucket's parts.

**Window** (ratified 2026-09-17):
The bucket a transfer broadcasts in. A transfer is due for the whole open
window. A window that closes without its broadcast is *missed*: the
transfer is rescheduled into a later window and counts the miss. Our term;
ZIP 318 speaks of the scheduled broadcast height.

**Scheduled broadcast height** (ratified 2026-09-17):
The block height inside the window a transfer aims its broadcast at, drawn
from the ZIP 318 delay law. Advisory: it aims the reminder and never gates
the broadcast.
_Avoid_: target height

**Window timeline**:
The chain's windows around the tip, each carrying the schedule's
confirmation progress there. Exists with or without a migration; the
current window is always present.

**Expiry bucket**:
The 30-day `EXPIRY_MODULUS` period a transfer's canonical expiry is
computed from; distinct from (and an exact multiple of) anchor buckets.

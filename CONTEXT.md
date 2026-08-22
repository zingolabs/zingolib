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
The private ZIP 318 flow: note splitting into denominations, then parts
transmitted across buckets.

**Note splitting**:
Phase 1 of the scheduled flow: Orchard self-sends that resize notes to
exactly denomination + part fee.
_Avoid_: note preparation (upstream's synonym; splitting is our term)

**Consolidation**:
Merging fragmented notes into fewer notes without crossing pools. Within
note splitting, the round that merges fragments before sizing.
_Avoid_: reduction

### Amounts

**Denomination**:
A canonical migration amount, {1, 2, 5} × 10^k ZEC. What the Shielded Labs
document calls amount "buckets". Never use bucket for amounts here.

**Part**:
One scheduled pool-crossing transaction carrying exactly one denomination.

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

**Window timeline**:
The chain's windows around the tip, each carrying the schedule's
confirmation progress there. Exists with or without a migration; the
current window is always present.

**Expiry bucket**:
The 30-day `EXPIRY_MODULUS` period a transfer's canonical expiry is
computed from; distinct from (and an exact multiple of) anchor buckets.

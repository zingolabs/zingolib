# zingolib

A Rust Zcash light-wallet library. The vocabulary below is the migration
domain (ZIP 318, Orchard → Ironwood); it follows the ZIP's language, with
gaps filled from the Shielded Labs migration-security recommendations.

## Language

### Migration paths

**Immediate migration**:
The non-private ZIP 318 option ("migrate immediately"): every spendable
Orchard note swept into one Ironwood output per transaction, real amounts
visible on-chain, broadcast at once.
_Avoid_: drain

**Scheduled migration**:
The private ZIP 318 flow: note splitting into denominations, then parts
broadcast across buckets.

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
document calls amount "buckets" — never use bucket for amounts here.

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

**Broadcast window**:
An upcoming bucket with parts due in it, as reported by migration status
for platform schedulers.
_Avoid_: wake, wake point

**Window timeline**:
The chain's windows around the tip, each carrying the schedule's
confirmation progress there. Exists with or without a migration; the
current window is always present.

**Expiry bucket**:
The 30-day `EXPIRY_MODULUS` period a transfer's canonical expiry is
computed from; distinct from (and an exact multiple of) anchor buckets.

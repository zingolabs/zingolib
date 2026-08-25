# A Destination Reservation levels concurrent Transmissions across operators

Status: draft — ratified in session 2026-08-10, pending review

## Context

Destination Selection carried two constraints — the sync indexer's
operator is never a transmit target, and a Transmission's Destination
must differ from the previous Transmission's — and both govern
consecutive sends. Nothing governed concurrent ones: two Transmissions
in flight at once could draw the same Destination, so the
non-accumulation property that Selection buys sequentially had no
concurrent counterpart on any platform. The 2026-08-10 audit surfaced
the gap alongside the Exit Pool comparison: exits already enjoy
absolute concurrent disjointness through the unique-per-holder
Reservation of ADR 0038, but the exit population is large and
fungible, while the Destination census is small — an absolute
exclusion there would make a send wait on, or refuse for, a transient
concurrency collision.

## Decision

The session keeps one Destination Reservation ledger, keyed by the
operator's registrable domain and never by endpoint URI, because two
endpoints of one operator are one observer. It governs the Indexer
kind only: a price run contacts its whole census and draws nothing.

The holder is the whole Transmission. Each arm's draw transfers one
reservation, a Transmission accumulates one per arm, and all of them
recycle together only when the Transmission completes — never when an
arm is cancelled, because an early recycle would let a concurrent
Transmission contact an operator still holding the first's
accepted-but-unconfirmed submission.

Issuance is tiered, not absolute. The draw is uniform over the
eligible operators carrying the fewest live reservations; the ratified
rounds emerge from that one predicate — while any eligible operator
holds none, only first reservations can issue, and a second round
opens only when every eligible operator holds one. The exposure this
accepts is stated plainly: an operator observes a second concurrent
Transmission only after every operator observes one, and so on upward,
with no artificial ceiling.

The absolute exclusions filter before the ledger. The sync operator,
the previous Transmission's Destination, and the operators this
Transmission already holds are removed first, and the Health floor
judges eligibility on Health evidence alone — reservations are
transient concurrency state, never evidence, so they neither push a
draw below the floor's anonymity argument nor masquerade as a
shrinking census. Rounds are therefore computed over the eligible
remainder: leveling never overrides a privacy exclusion to complete a
round.

## Considered options

Absolute exclusion with a wait on recycle, an outright refusal, and a
narrowed race that defers each widening until an operator frees were
all considered and superseded: each existed to answer an exhausted
ledger, and under tiered issuance the ledger always answers a draw.
The only remaining refusal is an eligible census that is genuinely
empty, which is a Health and census failure, not a concurrency one.

## Consequences

Concurrent Transmissions are pairwise disjoint whenever they number no
more than the eligible operators, and degrade uniformly beyond that,
which is the strongest claim a small census admits without stalling
sends. The mechanism is wallet-side and platform-blind: it needs no
acquirer, so it reaches mobile independently of ADR 0041's seam.
Implementation is a least-reserved predicate over the ledger, not
round bookkeeping, and the ledger's key is the Operator newtype of the
string-promotion census, which also ends the duplicated
operator-domain derivation in the sweep.

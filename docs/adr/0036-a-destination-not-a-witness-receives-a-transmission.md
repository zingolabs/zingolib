# A Destination, not a witness, receives a Transmission

Status: draft — ratified in session 2026-08-07, pending review

## Context

The send-privacy vocabulary named the indexer that receives a
Transmission its "witness," and the rotation property counted distinct
witnesses per Transmission. The 2026-08-07 session found the name makes
a false claim: a witness is by definition not a party — testimony comes
from outside the transaction — while this indexer is a principal in it.
It is served the submission, it relays or suppresses the transaction,
and it is held to account by the delivery check and the escalation.

## Decision

The role is named the **Destination**: the indexer a Transmission is
submitted to — the served party that receives the transaction, acts on
it, and thereby learns it. Witness Rotation becomes Destination
Rotation, Witness Selection becomes Destination Selection, and the
code sweep is scheduled as its own change.

The role was first minted as the Correspondent, taken from
correspondent banking — the institution that acts on another's behalf
at a remove. A 2026-08-25 amendment renames the role the
**Destination**, preferring the plain word for the addressed party
over the banking metaphor; the code, the ADRs, and the glossary were
swept the same day. _Avoid_: Correspondent.

## Considered options

Every candidate from the cited literature fails on a collision or a
missing facet. "Recipient" and "service provider" (the Nym whitepaper's
terms) name topology position and apply equally to the sync indexer and
a price source, so neither can carry the disclosure-accounting role.
"Counterparty" collides fatally with a payment's payee, the one
distinction a wallet must never blur. "Observer" is taken by the
Network Observer capability token. "Contact" is this domain's workhorse
verb ("distinct candidates contacted") and carries the address-book
sense of saved payees. Bare "witness" remains correct in exactly one
place and keeps it: the Merkle authentication path inside sync code,
where the sense is upstream-aligned.

## Consequences

The threat model lives in the glossary entry's body, not the name: a
censoring Destination accepts a submission and suppresses or
misreports the relay, and Destination Rotation bounds how many
distinct Destinations ever learn one transaction. The rename sweep
(WitnessAttempt, the witness fields and prose in the escalation module,
MAX_BROADCAST_WITNESSES jointly with ADR 0037's sweep) is scheduled and
tracked in the issue queue.

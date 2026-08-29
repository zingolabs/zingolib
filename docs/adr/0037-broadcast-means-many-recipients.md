# Broadcast means many recipients

Status: draft — ratified in session 2026-08-07, pending review

## Context

The word "broadcast" named two targeted submission paths: the send
escalation module and the Ironwood migration-part path, whose glossary
entry deliberately kept ZIP 318's own word. The 2026-08-07 session
examined the mechanism on both paths and found the same fact: each
submission goes to exactly one drawn Destination on the happy path,
and even failure escalation contacts a bounded few, winner-take-all.
The wider networking convention reserves broadcast for many-recipient
delivery — the "broad" announces the very property these paths are
designed not to have.

## Decision

"Broadcast" is reserved for genuinely many-recipient delivery. No
submission path in this wallet broadcasts, so the word leaves the
minted vocabulary: the send path speaks of Transmissions to
Destinations, and the migration-part path submits to the Migration
Transmission Endpoint. This deliberately breaks with ZIP 318's word for
part submission; ZIP fidelity yields to arity truth, and the departure
is recorded in the glossary's Broadcast entry.

The replacement is also grammatically complete where "broadcast" was
not: transmit is the verb and Transmission is the noun, so every use
site names its part of speech. "Broadcast" serves as both verb and
noun in one ambiguous form, and the sweep resolves each occurrence to
the correct member of the pair.

The distinction between the two submission paths survives on facts,
not words: a Transmission verifies the server-reported txid echo, the
migration-part path does not, and the part path keeps its decoupled
endpoint so the synchronization server cannot correlate the two
activities.

## Consequences

A rename sweep is scheduled and tracked in the issue queue: the
escalation module and its FanoutError, the Broadcast Indexer term, the
migration_broadcast_uri config key, the BroadcastClient family, the
NoEligibleBroadcastIndexer error, MAX_BROADCAST_WITNESSES (jointly
with ADR 0036's sweep), and the ZIP 318 scheduling term Broadcast
window. Upstream wire names the wallet does not own, such as
SendTransaction, stay as they are. Should a genuine many-recipient
mechanism ever appear, broadcast is its reserved and only name.

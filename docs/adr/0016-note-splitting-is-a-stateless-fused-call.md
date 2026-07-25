# Note-splitting execution is a stateless, send-shaped call

Status: accepted. Scoped to Phase 1 (note splitting); supersedes the
note-splitting role of the `continue_note_splitting` / `MigrationState`
driver for consumers. Phase 2 (parts, buckets, their consent binding) is
unaffected.

`LightClient::quick_split(account, resume_sync)` is the consumer entry point
for executing Phase 1 note splitting. Like the immediate Drain (ADR 0015) and
`quick_send`, it pauses sync internally, plans against the wallet's *current*
confirmed notes without synchronizing, and restores the prior sync mode on
return unless `resume_sync` is `false`. It does **one round** of Orchard
self-sends per call and returns a `SplitOutcome` — `Round { txids }`,
`AwaitingConfirmation`, or `Complete`. The consumer loops — sync,
`quick_split`, repeat — until `Complete`. Preview with `plan_note_split(account)`;
observe per-transaction progress through `split_progress_handle()`. Note
splitting persists **no** migration state: no `MigrationPhase` for Phase 1, no
stored round counter, no consent-hash binding. It refuses with
`MigrationError::AlreadyInProgress` while a *scheduled* migration is active:
that flow drives its own splitting and reserves notes for its parts, which the
fused path must not race.

## Why

Note splitting is a pure function of the wallet's current note set:
`plan_migration` replans the whole job from the account's spendable Orchard
notes and `quick_split` executes its first round, and a note already sized
`denomination + part fee` is never re-split. The whole job is therefore
recoverable by replanning — the same property that lets the Drain be re-called
after a partial broadcast. That makes a persisted driver unnecessary. The prior
surface (`start_ironwood_migration` → `continue_note_splitting` returning
`SplitStep`, backed by
`MigrationPhase::NoteSplitting { round, pending_txids }`) forced zingolib to
persist a phase machine and forced zingo-mobile — the primary consumer, across
UniFFI — to track and resume that phase. "Which round" is emergent (the
consumer's loop count), not state worth persisting.

## Trade-off

Two things the stored phase provided move or disappear:

- **The inter-round confirmation barrier moves to the consumer.** A round's
  shielded outputs must confirm and be witnessed before the next round can
  spend them, so `quick_split` does one round and the consumer syncs to
  confirmation before calling again. `quick_split` checks for an in-flight
  round **before it plans** and returns `AwaitingConfirmation` if one is found.
  This is load-bearing, not just a nicety: while a round is unconfirmed its
  inputs have left the spendable set and its outputs are not yet spendable, so
  a replan sees nothing left to split and would report a false `Complete`. The
  in-flight signal is **derived** from the wallet's pending transactions —
  specifically the round's own unconfirmed, account-owned V2 Orchard
  *outputs*, which the wallet records at build time, before transmit —
  replacing the stored `pending_txids`.
- **No consent-hash binding for Phase 1.** Splits reveal no value (both ends
  shielded) and may run before NU6.3 activation over the ordinary connection,
  so the `plan_note_split` preview is the disclosure surface, exactly as the
  Drain's plan is. Phase 2's parts still carry their own consent binding and
  state; this decision does not touch them.

## Considered and rejected

- **An explicit propose→send pair** (`propose_note_split` → `send_note_split`,
  mirroring `propose_send` / `send_stored_proposal`, ADR 0006). It
  guarantees the executed round is exactly the previewed one by threading a
  proposal value. Rejected: once the round counter dissolved, a back-to-back
  preview and execute on the paused wallet already see identical notes, so the
  guarantee buys almost nothing over the fused form, which is lighter and
  matches `quick_immediate_migration` verbatim. A `Proposal` also cannot span rounds — its
  multi-step form chains *transparent* outputs by outpoint, while a split's
  shielded outputs have no witness until mined — so the explicit form would
  still be one proposal per round.
- **Keeping `continue_note_splitting` / `MigrationState` for consumers.**
  Rejected for Phase 1: it persists a state machine that the memoryless replan
  makes redundant and that the mobile FFI would have to mirror and resume.

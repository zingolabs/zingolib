# Indexerless operations are pure functions; effects live at the edges

The Indexerless capability set (ADR 0001; the library glossary in
`zingolib/CONTEXT.md`) is exposed as value-returning pure functions over
wallet state: proposing takes wallet state and a transaction request and
returns a `Proposal`; calculating takes wallet state and a proposal and
returns signed transaction bytes for later broadcast. These operations do
not mutate the wallet. The effects a wallet application genuinely needs —
file I/O, broadcast, and any state a multi-call protocol requires — belong
to the shell that hosts the library: zingo-cli's `main`, or zingo-mobile's
UniFFI bridge crate.

This retires the stored-proposal pattern, in which `propose_send` wrote a
`ZingoProposal` into the `LightWallet` and `send_stored_proposal` consumed
it. That pattern reports failure honestly only through typed errors, and it
holds protocol state in a hidden channel: the wallet mutates on propose,
and whether a send is possible depends on state no signature reveals. The
retirement is follow-up work atop the rebased offline-mode branch (#2371),
not part of the rebase itself.

Two commitments follow. First, typed errors are a co-requisite, not a
separate cleanup: a pure function that smuggles failure prose into its
success value is not pure in any useful sense, and an Indexerless client
makes network-operation failure the routine path rather than the
exception, so those failures must be distinguishable by type
(issue #2446; the Least Authority audit's in-band-error finding).
Second, zingo-mobile's two-call send protocol (`send` then `confirm`)
keeps its FFI shape: the bridge holds the returned proposal in its own
slot, beside the `LightClient` global it already owns, and `confirm` on an
Indexerless client fails with the typed error while the slot retains the
proposal for retry.

## Considered Options

Keeping the stored-proposal flow for online callers while adding pure
entry points would have preserved mobile's code unchanged, at the cost of
a permanent dual architecture in which the same operation exists with and
without hidden state. Passing the proposal across the FFI as a serialized
value would make the mobile flow value-oriented end to end, but it changes
the UDL and every React Native call site, and transaction expiry
(~40 blocks) caps the useful lifetime of a carried proposal at under an
hour, so the durability it buys is illusory; that shape is reserved for
the future calculate/broadcast endpoints, whose natural payload is the
signed transaction. Persisting proposals across process restarts was
rejected outright and recorded as a non-goal: the wallet reader has always
reset `send_proposal` to `None` on load, and a proposal's target height
and note selection both decay, so the durable objects are the send intent
(re-proposed fresh) and signed transaction bytes — where support for
long-lived send intents might eventually live is deliberately left open.

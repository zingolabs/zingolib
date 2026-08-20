# Roles key conduits, and the wallet stops naming exits

Status: draft, ruled in session 2026-08-18, pending review and
implementation

## Context

ADR 0045 gave a boot four proven exits and named four exit roles. Its
glossary entry states that the role belongs to the exit rather than to any
client, because a client is the transient instrument that binds an exit
while the exit keeps its role and its proof for the epoch. That made exits
first-class wallet vocabulary by design.

ADR 0046 then ruled that the wallet asks for a conduit by role and never
learns how the request is served. Exits, clutches, and SOCKS5 addresses
belong below the seam, and the wallet holds an opaque handle.

The two rulings contradict each other, and the tree currently follows
0045. `ExitNodeId` appears 82 times in zingolib. `Pools` holds a
`HashMap<Role, ExitNodeId>`. The health index is keyed by exit, and the
spare's failure trigger charges tunnel-phase failures against one.
`MixnetStatus` carries a public list of exits, which zingo-cli renders to
the user through `render_exit_nodes` after `network on`.

So ADR 0046 has been half kept. Commit 03d8c7d01 moved the types into
`zingo-netutils`, and 5318ff405 replaced `SlotTunnel` with
`MixnetConduit`, but the wallet still names both. The move was a
relocation. The encapsulation the ADR describes has not happened, and it
cannot happen while a role binds an exit above the seam.

## Decision

A role keys a conduit. `Pools` stops holding a role-to-exit map, and the
wallet holds role-to-conduit instead.

The Exit Pool, the health index, and failure attribution go below the seam
with the exits they name. The provider chooses which exit serves a role,
remembers that binding for the epoch, and answers a role-keyed request
with a conduit over it.

zingolib stops re-exporting `ExitNodeId`. The wallet cannot name an exit,
which is what makes the opacity real rather than stated.

What a user is shown survives. The conduit carries a label, and the
session's status reports labels rather than typed exit identities, so the
`network on` report still tells a user what they are bound to.

## Considered options

**Keep exits above the seam and accept partial opacity.** The smallest
change, and it matches the tree today. Rejected because it makes ADR 0046
aspirational: a decision record that describes a property the code does
not have is worse than no record, since a later reader trusts it.

**Move `Role` below the seam so the provider keys on it directly.** This
resolves the contradiction by relocating the other half. Rejected because
a role names a wallet job. The wallet decides that a Server-Selection
Sweep and a price fetch are different work, and a crate that knows nothing
of sweeps or prices should not own that enumeration.

**The chosen split.** The role stays above the seam because it names a
job, the exit goes below because it names an implementation, and the
conduit is what crosses. This is the only division where each name sits in
the crate that can justify it.

## Consequences

ADR 0045's **Exit Role** glossary entry is amended rather than retired.
Below the seam the role still belongs to the exit, exactly as 0045 says,
and the provider is what knows it. Above the seam the role belongs to the
job, and no exit is nameable.

The 82 references in zingolib move below the seam or disappear. This is
the bulk of the work and it is not mechanical, because several of them
carry policy rather than data.

`charge_phase` keeps typing which party a failure charges, but the tunnel
arm no longer names an exit above the seam. Attribution against a specific
exit becomes the provider's bookkeeping, and the wallet keeps only what it
can act on, which is whether its conduit still serves.

The spare's failure trigger becomes provider policy. So does the count of
four, which ADR 0046 already moved there.

`render_exit_nodes` takes labels rather than `ExitNodeId` values, and the
status surface both frontends read carries the same.

The mobile attach path gains the role-keyed request it lacks today, which
is also what goal (b) needs to establish proven exits early on a phone. The
two goals converge on this one change rather than competing for it.

Two questions are deliberately left open. Whether a conduit's label stays
stable for an epoch is a display decision, not a structural one. What the
session reports when a role's exit changes mid-epoch depends on that
answer.

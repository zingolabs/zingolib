# A mobile session rotates one client rather than separating roles

Status: draft, ruled in session 2026-08-18, pending review and
implementation

## Context

ADR 0045 gives a boot four proven exits and a role for each, so that no
exit sees two kinds of traffic. ADR 0046 then made the count a provider's
choice rather than an architectural commitment, and ADR 0047 made a role
key a conduit. None of them says what a phone should do.

A Nym client is not idle when the wallet is. It holds a persistent
connection to its gateway and generates cover traffic continuously,
because that is how a mixnet resists traffic analysis. Four clients means
four times that, for the whole session, on a battery and often on metered
data. The boot cost is known and modest: an exit announces in a mean of
4637 milliseconds with a standard deviation of 549 over 30 births, a
Sentinel round trip averages 1253 milliseconds, and 30% of exits carry
nothing, so one proof costs about 5.9 seconds and succeeds first time 70%
of the time. The standing cost is unmeasured, and it is the constraint.

Today a mobile session attaches one endpoint and keeps it for the session's
whole life. That is the worst of both worlds. It has neither the separated
scope ADR 0045 buys nor any bound on how long a single exit observes the
wallet.

One premise deserves correcting, because it has been assumed in
discussion. Exits are not unique to an epoch. A network requester's Nym
address is a stable bonded-node identity, and the API's own arithmetic
shows key rotation spanning `validity_epochs` epochs rather than one. An
epoch boundary rotates the active set and the layer assignment, so an exit
may leave and return, but it does not become a different exit. Rotation is
therefore a privacy choice and never an expiry requirement.

## Decision

A mobile session runs exactly one `Socks5MixnetClient` at a time.

That client rotates on a randomised interval between five and ten minutes,
and each rotation binds a different exit.

Rotation is make-before-break. The replacement bootstraps and proves before
the retiring client stops, so the app always has service and a rotation is
never visible as an outage.

An in-flight Transmission completes on the retiring client. A hand-off
changes what new work uses and never what running work uses.

Mobile's provider answers every role request with the current conduit.
Roles still key conduits exactly as ADR 0047 says. On a phone they all key
the same one, and what changes is which exit that one conduit reaches.

The mechanism belongs to `zingo-netutils` and the policy belongs to the
host. Proving a replacement, holding both clients through the hand-off, and
draining in-flight work are subtle and identical on every platform, so they
are written once below the seam. Deciding whether now is a moment to spend
a bootstrap needs the battery level, the foreground state, and whether the
radio is on wifi or cellular, and none of those are observable from this
workspace. A wallet cannot make a resource-constrained decision it has no
inputs for.

That split is expressed as a trait `zingo-netutils` defines and the
platform implements, in the shape `ProxyHosting` already has: the host
supplies a transport when asked and answers whether a rotation is welcome
now. The cadence bounds stay in `zingo_netutils::time` and reach the host
through `mixnet_timing`, so neither side pins its own copy of a number the
other enforces.

## Considered options

**Four clients, as the desktop has.** Gives mobile the separated scope
ADR 0045 designed. Rejected on standing cost: four continuous cover-traffic
streams is a battery and data cost a phone notices and a desktop does not.

**One client, no rotation.** What ships today, and the cheapest. Rejected
because a single exit then observes an entire multi-hour session, which is
a worse privacy position than either alternative.

**One client, rotating.** Chosen. It trades separated scope for bounded
exposure, which suits a session that runs for hours on a device the user
carries.

## Consequences

The privacy property changes rather than degrades, and the trade should be
stated plainly wherever this is documented. Role separation stops one exit
from linking a price fetch to a Transmission. Rotation stops any exit from
seeing more than ten minutes of the wallet. Within a window the rotating
exit sees everything, so a send that coincides with a price fetch is
linkable exactly as role separation was meant to prevent. Sends are rare
enough that the coincidence is uncommon, and it is also the moment that
matters most to a user, so the residual risk is small but real.

Against an adversary who runs exits and accumulates observations, bounded
exposure is the stronger property for a long session. Under role
separation the Transmission exit sees every Transmission for a whole
epoch. Under rotation no exit sees more than one window of them.

The hand-off overlap runs two clients briefly. At about 5.9 seconds of
proving against a five-to-ten-minute cycle, that is one or two percent of
the session at doubled cover traffic, which is a fair price for never
dropping service.

Six to twelve full client bootstraps an hour is the cost this decision
actually incurs, and each is a gateway registration and a topology fetch
rather than a cheap reconnect. It is unmeasured on a device and belongs in
the standing-cost measurement alongside idle cover traffic.

The rotation cadence is a privacy parameter. It gets a named constant and
a glossary entry, never a literal at a call site.

Desktop is unchanged. ADR 0045's four roles stand where a client costs
nothing a user notices.

Mobile moves from a push seam to a pull one, and that is the interface cost
of this decision. Today the platform starts a client and hands the wallet
its address through `attach_mixnet`, which the wallet can only accept.
Under a supply-on-request seam the platform answers when asked, which is
what lets the mechanism live below the seam and run the hand-off. The two
can coexist during migration, since a first attach and a hand-off are
different intents.

`attach_mixnet` cannot express a hand-off as it stands. It calls
`vacate_mixnet_slot` before installing, so it stops the serving client
first, and it asserts the slot was empty afterward. `install_failover_client`
is the only path that replaces an attached client and it requires the
incumbent to be condemned, which a healthy rotation's never is. Both need
an entry point that installs the replacement and retires the superseded
client once its work has drained.

## Open

Whether rotation should pause while the app is backgrounded and idle.
Rotating with no traffic to unlink spends battery to hide nothing, so the
cadence may want to follow activity rather than the clock. This is exactly
the kind of judgement the policy trait exists to delegate, so it may need
no answer here at all.

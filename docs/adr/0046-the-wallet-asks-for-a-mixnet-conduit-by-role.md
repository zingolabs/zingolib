# The wallet asks for a mixnet conduit by role

Status: draft — ruled in session 2026-08-18, pending review and
implementation

## Context

ADR 0045 gave a boot four proven exits and a role for each. It did not
say who owns the vocabulary those roles are written in, and the answer
today is the wrong crate. `zingolib/src/mixnet.rs` defines
`pub struct ExitNodeId(String)`, and the wallet crate goes on to own the
Exit Pool, Reservations, births, the clutch, the quartet, and
`SlotTunnel`. `zingo-netutils` owns the implementation those names
describe: `NymProxy` with the nym-sdk resolved in its own lockfile, the
Sentinel, the SOCKS5 dialing in `socks5_transmit`, and the timing
constants. The seam runs between the crates, and Nym's vocabulary has
crossed it in the wrong direction.

The mobile platform makes the cost concrete, because the two boots are
opposites rather than variants. On the desktop, `enable_mixnet_from`
calls `prove_quartet` and the wallet births four clients drawn from its
own Exit Pool. On mobile, `attach_mixnet` receives one SOCKS5 address
and a candidate list from the host, births nothing, holds no
reservation, and is born already proven. A wallet written against the
desktop shape cannot express the mobile one, so the mobile session gets
a single endpoint and none of ADR 0045's roles.

Proof is not free, which is why the count matters on a phone. Sixty
births measured against mainnet on 2026-08-18 announced in a mean of
4637 milliseconds, and thirty percent of the exits that announced
carried nothing. A boot that insists on proving four exits before it
opens a prompt spends four clients' worth of battery, memory, and
bandwidth to buy failure isolation the phone may not want.

## Decision

The wallet asks for a conduit by role and never learns how the request
is served. `MixnetConduit` is an opaque handle, defined in
`zingo-netutils` beside the provider that mints it. The wallet holds
one, names one, and passes one back to have work carried through it. It
cannot read an address out of it.

Roles stay wallet vocabulary and stay in `zingolib`, because the wallet
is what decides that a Server-Selection Sweep and a price fetch are
different jobs. Everything the role is served *with* — exits, births,
clutches, proxies, the Sentinel, and the number four — stays below the
seam.

The provider decides how many mixnet clients a set of roles takes. A
provider that answers every role with the same conduit is serving them
all over one client. A provider that answers with distinct conduits is
serving each over its own. Both satisfy the same wallet code, and the
wallet cannot tell them apart.

Mobile takes the first shape. Its provider answers every role with the
host's single proven endpoint, so time to responsiveness is one proof
rather than four, and the sweep and the price fetch begin against a
conduit that is already proven when the session opens.

## Considered options

**The host proves four endpoints and hands over four**, mirroring the
desktop quartet. Rejected because it writes the number four into the
mobile contract, which is precisely the implementation detail this
decision removes. A later provider that serves four roles with two
clients would have to break the contract to do it.

**The host hands one endpoint and the wallet multiplexes every role over
it.** This is a sound policy and was nearly adopted. One endpoint does
not serialize the roles: the Server-Selection Sweep already drives a
whole cohort of candidates through a single SOCKS5 address, sixteen of
seventeen in a measured run. It was rejected as an *abstraction* rather
than as a policy, because the wallet would still hold the address and
still assume SOCKS5. The chosen decision subsumes it — a provider
answering every role with one conduit is exactly this option.

**Naming.** `ExitAnchored` was considered and rejected on two grounds.
It reimports exit vocabulary into the one type this decision exists to
keep free of it, and it is a predicate where Rust type names are nouns.
The property it names is true and is recorded in the type's
doc-comment instead. `provider::Conduit` was considered, taking the
domain from the module path, and rejected because "provider" does not
say what is provided and module names will shift as the wallet's service
contract takes shape.

## Consequences

`ExitNodeId`, the Exit Pool, Reservations, births, the clutch, and
`QUARTET_SIZE` must move from `zingolib` into `zingo-netutils`. This is
the bulk of the work and it is mechanical, but it is not small, and it
cannot be done in one step without leaving the tree broken.

`SlotTunnel` is superseded by `MixnetConduit`, so the excision of the
term "tunnel" from the wallet's vocabulary lands with this decision
rather than as separate work.

The wallet loses per-exit failure attribution as it stands today.
`charge_phase` types the party a failure charges, and one of those
parties is the exit, which the wallet will no longer be able to name.
Attribution has to move below the seam with everything else, and the
wallet keeps only the part it can still act on: whether its conduit is
serving.

The wallet stops dialing. `Socks5Indexer` already dials below the seam,
so the wallet hands over a target and receives a result rather than
receiving an address and connecting itself.

The glossary entry for **Exit Role** says the role belongs to the exit
rather than to any client. That remains true below the seam and becomes
invisible above it, where the role belongs to the job. The entry needs
amending rather than retiring.

Two questions are deliberately left open. Whether the desktop boot keeps
proving four exits is now a provider policy and no longer an
architectural commitment, so it can be decided by measurement later.
How a conduit carries work — what the wallet passes back with it — is
the next decision and is not settled here.

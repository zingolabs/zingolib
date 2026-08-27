# IP obfuscation for Transmission and price-fetch runs over the Nym mixnet

To keep a server-side adversary (principally the indexer) from learning the
client's IP address and linking it to a wallet's activity, we route the two
highest-linkage outbound surfaces, Transmission and price-fetch, through the
Nym mixnet, while leaving synchronization on the ordinary connector. The
motivation is fund protection: a broadcast is a transaction submitted *to the
indexer*, so an indexer that sees the client's real IP at broadcast time, or
that has already fingerprinted the IP against the wallet's address set during
sync, can tie a person to a transaction. The mixnet hides the client's IP from
every service it fronts, which is the property that broadcast most needs.

## The adversary and why the mixnet, not Tor

The named adversary sits server-side at any service the client contacts; a
mixnet exit gateway seeing the *destination* is acceptable, because the
service then sees only the gateway. We chose the Nym mixnet over Tor
deliberately. Send has never in fact run over Tor: the historical Tor support
was wired only to price fetching and was removed in June 2026, so there was no
incumbent to preserve. Send over Nym, by contrast, was proven in an unmerged
May 2026 proof of concept, and the mixnet's per-packet mixing defends against
the timing correlation that ordinary onion routing does not. We accept the
mixnet's higher latency because the surfaces that carry it are small and
infrequent.

## Per-surface transport tiers

Obfuscation is a property of each surface rather than of the whole client, and
two transports run at once. Transmission and price-fetch require the mixnet.
Synchronization (including pepper-sync's long-lived mempool stream) does not;
it may run over bare clearnet or, for a user who wants the indexer blinded to
their IP during sync as well, over a system-provided NymVPN. Sync degrades
gracefully and never fails closed. This tiering spends the expensive anonymity
only where the linkage is most dangerous and spares the continuous, latency-
sensitive sync stream from mixnet cost. A consequence follows and is recorded
plainly: bare-clearnet sync leaks the real IP and the wallet's address set to
the very indexer that Transmission is hiding from, so the coherent fully-
protected posture is NymVPN sync paired with mixnet send, and bare clearnet is
the explicit "do not care" tier.

## The dependency exception

Enabling this stack requires the Nym SDK and its companions
(`nym-sdk`, `nym-http-api-client`, `nym-validator-client`, `tokio-socks`, and
`tower`) to enter the in-workspace `zingo-netutils` crate. This is a ratified
exception to the standing rule against new dependencies, granted specifically
because these crates are permissively licensed and published on crates.io, and
narrowed by placing the whole stack behind an off-by-default `nym` feature so
that default, mobile, and CI builds remain free of it. No `[patch]` tables and
no fork or branch pins are authorized; the crate having moved in-workspace, the
proof of concept's `nym_proxy.rs` ports directly with no external pin.

The nym exception carries a transitive TLS-backend exception, ratified
2026-07-21. This workspace's crypto backend is aws-lc-rs, never ring — every
backend we choose selects aws-lc-rs — but `nym-sdk` transitively links ring
through its cosmos credential core (`nym-client-core` → `cosmrs` →
`tendermint-rpc` → `reqwest 0.11` → `hyper-rustls 0.24` → `rustls 0.21`, and
rustls gained the aws-lc-rs backend only in 0.23). It sits in the mixnet
client's core, so no feature flag severs it, and cutting it would require
forking or `[patch]`-ing nym, both barred. The ring is accepted as unavoidable
and confined to the standalone `zingo-netutils` nym build — the proxy binary
and the mobile UniFFI shim — never entering the main wallet lock, whose only
ring is a separate build-time transitive of `zcash_proofs`'s parameter
downloader and predates this work. Eliminating both is tracked upstream, not
undertaken here.

## NymVPN is provided by the user, never embedded

The NymVPN layer that protects the sync tier is installed and run by the user
at the operating-system level; the wallet does not embed it. Three independent
facts forbid an in-process link. The NymVPN client stack is GPL-3.0 while every
zingolib crate is MIT, so static linking would relicense the distributed
wallet. Its crates are unpublished, reachable only by a git pin the dependency
rule forbids. And the tunnel needs a system TUN device that a wallet process
cannot create (on Android it is the single consent-gated `VpnService`, on iOS
a separate entitled Network Extension), so an embed is impossible on the mobile
targets that matter most. The low-latency mode we would want for sync is
moreover NymVPN-exclusive: the mixnet SDK is fixed at five hops and exposes no
faster tier, so no shortcut through the already-approved SDK exists. A thin,
desktop-only control client that speaks `nym-vpn-proto` gRPC to a user-run
daemon, linking no GPL code, is reserved as a possible later refinement for
callers who want the wallet to guarantee the tunnel is up before syncing.

## The seam, and how a send behaves

The synchronization client stays exactly as it is; a separate component, built
from a single shared mixnet proxy, owns the two mixnet surfaces. Routing is
decided by each operation's static tier, never by a per-call boolean, so no
call site can accidentally route a send over the wrong transport. A send picks
one indexer at random from a curated broadcast list of eleven reliable,
low-latency indexers (a list kept separate from the sync-server list, because
broadcast wants reliable relay and sync-ranking wants low query latency), and
submits over the mixnet. Those eleven are the mixnet-reachable subset of the
fourteen distinct operators the 2026-07-21 discovery sweep found; the other
three were excluded because they answer only on port 9067, which the mixnet
exit gateways cannot reliably relay. The purpose is witness rotation: because
the indexer that carries any given send is random, no single indexer
accumulates a picture of all the user's sends, and the broadcast target is
decoupled from the address-knowing sync indexer.

Delivery is pursued through an escalating, serially gated fan-out whose purpose
is robustness to censorship. The adversary here is a Broadcast Indexer that
suppresses a send (accepting the connection but declining to relay the
transaction, or misreporting the outcome), and the countermeasure is the
ability to route the same send around it to honest indexers. The first round
submits to a single random indexer, so the common case (a send that lands on
its first try) contacts exactly one witness and keeps the rotation property
intact. Only if that submission fails to confirm delivery does the send
escalate: the second round submits to two fresh random indexers in parallel,
and the third round, entered only after both of the second round's submissions
fail, submits to three more. Each round is gated on the complete failure of the
round before it, so parallelism widens only as evidence of censorship or
failure accumulates, and the fan-out stops at a cap of six distinct indexers,
which the one-two-three schedule reaches at the end of the third round. Within
a round the first indexer to confirm delivery wins and the rest are abandoned.
This supersedes the project's earlier "never fired redundantly, exactly one
submission in flight" rule: redundancy is now accepted on the failure path as
the price of censorship resistance, while the happy path keeps its
single-witness discipline.

Because the client cannot assume whether zainod, lightwalletd, or another
server sits at the far end of the mixnet, failover is driven by attempt counts
and delivery checks, never by classifying a server's error text. A round
"fails," and so escalates, on any outcome short of confirmed delivery (a
transport failure, a refusal, or a silence), since a censoring indexer can
present any of these. Success is defined by delivery: an accepted submission, a
duplicate already in the mempool or chain, or a delivery check that finds the
transaction known. Because a censoring indexer can also misreport delivery, the
strongest confirmation is one that does not rest solely on the word of the
indexer being tested (a cross-check against a different indexer or the
client's own sync view of the public mempool), and the implementation should
prefer such an independent check where it is available. There is no
merit-rejection short-circuit; an unbroadcastable transaction is bounded
instead by the six-indexer cap, after which the send surfaces failure for the
user to retry, which reshuffles the list. The retry, duplicate-in-mempool, and
queued-for-download handling for each individual submission is the one
resilient-transmission policy already shared with the clearnet path; the
fan-out orchestrates that shared per-indexer policy across rounds rather than
duplicating it. One persistent
mixnet client serves every send, since the indexer never sees the Nym client
identity and a fresh client per send would pay the gateway-registration cost
needlessly; each send nonetheless takes fresh reply isolation within that one
client as cheap defense in depth against a network-level observer.

## The toggle and its fail-closed invariant

The mixnet is controllable at runtime through a `LightClient` API that both
zingo-mobile and zingo-cli drive, exposing a tri-state status (off,
bootstrapping, or ready), because "on but not yet reachable" is a real state a
user interface must show. The mixnet is forced on at the start of every
connected session and its off-state is never persisted, so the worst case is a
user re-disabling it rather than a forgotten-off clearnet broadcast; an
`--offline` session, which never transmits, skips the bootstrap entirely. The
invariant that protects funds is that a clearnet send happens only as the
user's deliberate, per-session choice: when the mixnet is on and its transport
fails mid-send, the send refuses rather than silently dropping to clearnet.
Informed consent is permitted; silent degradation is not.

## Testing

The mixnet is unreachable from CI, so every piece of send, toggle, and
fail-closed logic is tested there against an injected mock transport and a
seeded, injectable random-number generator; the real proxy bootstrap and a live
transaction over the mixnet are a single feature-gated smoke test run by hand.
This makes injectability of the transport and the generator a binding
constraint on the design rather than a convenience.

## Considered options

We rejected routing send over Tor, which never carried a send here and offers
weaker correlation resistance than a mixnet. We rejected embedding NymVPN for
the license, distribution, and mobile-OS reasons above. We rejected sourcing
price from an on-chain oracle (which would have eliminated the price surface
rather than obfuscating it) because it would make the wallet depend on an
oracle that does not yet exist; price-fetch therefore rides the mixnet like
send, reusing the same proxy through the HTTP client's SOCKS5 support.

## Consequences

The Nym stack grows the dependency tree, contained behind the `nym` feature.
NymVPN's low-latency gateways require a paid credential, so a fully-protected
sync posture costs the user money and setup while mixnet send and price cost
nothing; bare-clearnet sync will consequently be the common real-world posture,
which makes the indexer-correlation caveat a live concern to document rather
than a corner case. The mixnet bootstrap imposes a startup latency on connected
sessions, and a send attempted during bootstrapping must wait or report
"connecting," never silently clearnet or silently fail.

## Amendment (2026-07-16): the mixnet proxy is a spawned child process

The description above of "a separate component, built from a single shared
mixnet proxy" implied the proxy is linked into the wallet process. Dependency
resolution forbids that. The nym-sdk stack requires `crypto-common ^0.2`,
which cannot share a `Cargo.lock` with this workspace's `crypto-common
=0.2.0-rc.1` pin, reached through `zcash_primitives` 0.29; both requirements
fall in the same `0.2.x` compatibility range, so a single lockfile can hold
only one and satisfies neither side. `zingo-netutils` was therefore made its
own workspace with its own lockfile, and the mixnet transport genuinely
cannot be linked into the wallet.

The ratified model is out-of-process. `zingo-netutils` builds a `nym-proxy`
binary in its own lockfile that runs the embedded Nym SOCKS5 client and
prints its local SOCKS5 address. The wallet bundles that binary, spawns it as
a child process, reads its address, and routes the Transmission and
price-fetch surfaces through it with a light SOCKS5 client (`tokio-socks`)
that resolves cleanly in the main lockfile. The tri-state Mixnet Mode maps to
the child's lifecycle: off is not spawned, bootstrapping is spawned and
connecting, ready is SOCKS5-reachable. The fail-closed invariant is
unchanged. If the child never reaches ready, a send refuses rather than
falling back to clearnet.

Everything else in this record stands: the per-surface tiers, the
witness-rotation broadcast over the curated Broadcast Indexer list, the toggle
semantics, and the sync tier's user-provided NymVPN.

## Amendment (2026-07-17): the broadcaster is an escalating fan-out

The broadcast policy this record originally described (a single random pick
with sequential failover and "exactly one submission ever in flight") is
superseded. Its stated purpose was witness rotation for privacy; the amended
purpose adds robustness to censorship, since a Broadcast Indexer can accept a
send and then suppress the relay. A send now uses an escalating, serially gated
fan-out: round one submits to a single random indexer, round two submits to two
fresh indexers in parallel only after round one fails to confirm delivery, and
round three submits to three more only after both of round two's arms fail. The
fan-out is capped at six distinct indexers, which the one-two-three schedule
reaches at the end of round three. Witness rotation is retained on the happy
path (a first-try success still contacts exactly one witness), and redundancy
is accepted only once delivery is in doubt. The "The two mixnet surfaces"
section above has been rewritten to reflect this; the delivery-check and
no-error-string-classification discipline is unchanged, and each individual
submission still runs the one resilient-transmission policy shared with the
clearnet path.

## Amendment (2026-07-21): a fourth mode, and the proxy's lifetime coupling

The tri-state Mixnet Mode this record ratified is superseded by four states:
off, bootstrapping, ready, and died. A live stage-three smoke run exposed the
gap. The user interrupted a stuck command with Ctrl-C, the terminal delivered
the signal to the whole foreground process group, and the nym-proxy child died
silently, after which every fan-out arm failed against a proxy that the mode
still reported as ready. Died names that condition: the spawned proxy exited
unexpectedly, during bootstrap or after reaching ready. It is distinct from
off because it is unconsented. Off remains the only state that routes the
mixnet-only surfaces over clearnet, as the user's deliberate choice; died
refuses, and the refusal tells the user to re-enable the mixnet, which spawns
a fresh proxy. The supervisor's reader consequently watches the child for its
whole life rather than detaching once the address arrives, and a death clears
the stale SOCKS5 address so no surface can dial a dead proxy.

Two mechanisms bind the child's lifetime to the wallet session without letting
a terminal signal tear the transport out from under it. The child is spawned
in its own process group, so a Ctrl-C aimed at a wallet command no longer
reaches the proxy; the interrupt aborts the command while the mixnet keeps
running. And the supervisor holds the child's stdin pipe open for the child's
whole life while the child watches that pipe for end-of-file: any parent exit
(clean, panicked, or killed with SIGKILL, which skips ordinary kill-on-drop
cleanup) closes the pipe and the child disconnects from the mixnet and exits.
No orphaned proxy survives its parent, and no parent interrupt orphans a
session from its transport. On platforms without process
groups the child shares the terminal's group and an interrupt still reaches
it; the outcome is the safe one (the mode becomes died and the surfaces
refuse) rather than a silent clearnet fallback.

## Amendment (2026-07-21): the runtime boundary generalizes for mobile

The spawned-child consumption model is the desktop instance of a general
rule, not the rule itself. What the dependency conflict actually forces is
that the nym stack live in its own resolution unit and meet the wallet at a
*runtime* boundary that carries exactly two things: a local SOCKS5 endpoint
and a liveness signal. A child process is one such boundary. Mobile platforms
demand others, and the wallet core should not care which one is in play.

Three consequences, one per layer. First, zingolib gains an attach entry
point beside the spawn one: the platform hands the wallet an already-running
local SOCKS5 address, the mode reaches ready after a connectivity check, and
liveness is thereafter observed by a periodic probe of that endpoint — probe
failure lands died, preserving the refuse-never-clearnet invariant without
the stdout pipe. Everything downstream of "mode plus address" — the route
resolver, the fan-out, the price fetch, the status narration — is unchanged.

Second, Android keeps the spawn model. An app may execute a bundled binary
only from its native-library directory, so the proxy ships as a native
library named like one (`libnym_proxy.so`) and the existing supervisor,
stdin-EOF watchdog included, runs it from there.

Third, iOS cannot spawn processes at all, and two Rust static libraries
cannot link into one binary. There the boundary becomes a dynamic library:
the standalone netutils workspace — already a separate resolution unit with
its own lockfile — builds the proxy as a small UniFFI dynamic framework
(start returning the SOCKS5 address, stop, and a death callback), the app
hosts it, and hands the address to the wallet's attach entry. The proxy is
its own UniFFI component, distinct from the wallet's: UniFFI binds the
wallet layer (zingolib's toggle and status methods, which are plain strings
and enums) as one component, while the proxy is a separate component with
its own generated bindings. The two boundaries never meet in one interface
definition.

Amendment (2026-07-21): the mobile proxy boundary is UniFFI, not a raw C
ABI. An earlier draft of this decision specified three hand-written
`extern "C"` functions, deliberately not a UniFFI surface, for minimalism.
That is reversed. This repository requires every Rust file to carry
`#![forbid(unsafe_code)]`, and a hand-written C ABI cannot honour it: the
raw-pointer marshalling, the callback function pointer, and the
`CString`/`Box` `into_raw`/`from_raw` calls are all `unsafe`, which would
force a ratified exception to the rule and place hand-rolled unsafe in the
shim. A UniFFI surface does not. It was verified empirically that
`#![forbid(unsafe_code)]` does not fire on uniffi's proc-macro-generated
scaffolding — a control crate carrying the lint compiled a
`#[uniffi::export]` whose expansion holds dozens of `unsafe` occurrences,
while a single hand-written `unsafe` block in that same crate did trip the
lint. UniFFI is therefore the only option that keeps `forbid` intact with
no exception, contributing zero hand-written unsafe while the lint still
rejects any a future contributor introduces. The minimalism argument does
not outweigh preserving the safety invariant the whole codebase depends
on.

## Amendment (2026-07-22): a Broadcast Witness is never the sync indexer

Witness Rotation as ratified here left one draw unconstrained: nothing
prevented the random pick from landing on the very indexer the wallet
synchronizes against — the one party that already holds the address set,
and the named adversary of this decision. That gap is closed by ADR 0022,
which makes the exclusion a universal, code-enforced invariant: every
transmission draw filters the curated pool by the sync indexer's operator,
and an emptied pool refuses in keeping with the fail-closed rule. See
`docs/adr/0022-broadcast-witness-never-the-sync-indexer.md`.

## Amendment (2026-07-23): migration parts obey Mixnet Mode, and never target the sync server

Ratified while walking the PR #2470 review findings (M3). The original
record routed Transmission and price-fetch by Mixnet Mode but left ZIP 318
migration-part broadcasts on unconditional clearnet, silently breaking the
mode's central invariant for the wallet's most correlation-sensitive
traffic. Migration broadcasts now obey the same policy as every other
transmitting surface. While the mode is on (assuming the `nym` feature is
compiled in and the session has not opted out), parts travel ONLY over the
mixnet and MUST NEVER go over clearnet: the broadcast client resolves the
route first and fails closed while the proxy bootstraps or after it dies,
refusing rather than falling back. Clearnet carries parts only through the
user's deliberate per-session toggle-off, or in a build compiled without
the feature, where the historical behavior (the dedicated
`migration_broadcast_uri`, else the synchronization endpoint with a logged
correlation warning) is unchanged.

Over the mixnet, migration parts ride Witness Rotation like sends: each
submission draws one Broadcast Indexer at random from the curated list.
The synchronization endpoint is forbidden as a mixnet target in both
shapes of the draw (a configured `migration_broadcast_uri` sharing the
sync server's host is refused with a typed error, and the random draw
excludes that host from the list), so no single server can correlate a
wallet's sync stream with its migration cohort, which is the correlation
ZIP 318's scheduling machinery exists to prevent.

## Amendment (2026-07-23): the price fetch loses its clearnet tier

Ratified in the same review walk-through as the migration amendment
above, and stricter: the price fetch travels ONLY over the mixnet, with
no clearnet tier in any configuration. Unlike send, whose clearnet
opt-out exists because a user may need to move funds when the mixnet is
unavailable, the price fetch contacts a third-party price API whose
value is cosmetic, and a clearnet contact leaks the client IP and
wallet-alive timing to a party outside the Zcash ecosystem. There is no
availability argument, so there is no opt-out: while Mixnet Mode is off
the fetch is refused with a typed error naming the remedy, and while it
bootstraps or after the proxy dies it fails closed as before.

The gate is a single switch. zingolib's `nym` feature forwards
`zingo-price/socks5-fetch`, the only configuration in which any fetch
code exists: the fetch function requires a SOCKS5 proxy address, so
even an enabled build cannot express a clearnet fetch, and a default
build compiles no fetch at all. zingo-price's network dependencies
(reqwest and its TLS/serde companions) became optional behind that
feature, so the default build's dependency graph shrinks below its
pre-mixnet shape; the crate's price types and their wallet-file
serialization stay unconditional, keeping wallet files portable between
builds with and without the feature.

## Amendment (2026-07-27): the clearnet price tier is restored

The 2026-07-23 price amendment above is superseded by PR #2548
("fix/restore-price"), merged to `dev` on 2026-07-27. The price fetch
regains a clearnet default: `update_current_price` works in every
build, including builds without the `nym` feature, and its
documentation discloses that the contact leaks the client IP and
wallet-alive timing to the third-party price source. zingo-price
returns to unconditional network dependencies and becomes a pure
mechanism — the fetch function takes an optional SOCKS5 proxy address
and the routing policy lives entirely in the caller.

The mixnet route survives as the opt-in
`update_current_price_over_mixnet`, a `nym`-feature method that keeps
this record's fail-closed invariant: it refuses while Mixnet Mode is
off, fails closed while the mode bootstraps or after the proxy dies,
and never falls back to clearnet. Its success value remains
`MixnetPriceFetch`, carrying the tunnel endpoint the fetch traveled
through, so a consumer that chose the private route holds per-fetch
evidence of it.

## Amendment (2026-07-28): off decomposes into Unattached and SwitchedOff

The four-state mode the 2026-07-21 amendment ratified is superseded by five
states: unattached, switched off, bootstrapping, ready, and died. An
automated review of zingo-mobile PR #1225 exposed the gap. The wallet
derived its mode from the absence of a proxy handle — a never-attached
wallet reported off — while the route resolver maps off to clearnet as the
user's informed consent. On mobile, where the platform starts the transport
and a start can fail, the conflation opened a real path to unconsented
clearnet: the coordinator published its fail-closed failure view, the next
steady poll read off from the wallet that had never attached, the presenter
took off as consent, and the send gate opened about thirty seconds after
the failure the user was never asked about.

The root cause is representational: "no transport was ever established" and
"the user chose clearnet" are different facts that shared one variant. The
decomposition gives each its own state. Unattached names a present
condition, not a history: no transport is established and no consent is
recorded. It is the initial state, and equally the state after a failed
enable or re-enable — a wallet that once ran a transport returns to
unattached when a fresh enable fails, because refusal follows from the
current absence of transport and consent, never from history. It refuses
the mixnet surfaces exactly as bootstrapping and died do, because absence
is not consent. A failed enable never restores an earlier switched-off
state either: by enabling, the user revoked the standing clearnet consent,
and a failure must not silently reinstate it. SwitchedOff is
reached only by the explicit disable call and remains the sole
clearnet-routing state; the rename from Off makes the deliberate act part
of the name. The wallet owns the distinction as an explicit state field
rather than deriving it from `Option` on the proxy handle, since dropping
the handle on disable would erase the very bit that separates the two
states.

The considered alternative — the mobile coordinator tracking a
session-local consent bit and withholding trust from polled off — was
rejected because it patches one consumer while every other reader of the
mode keeps consuming the lie: the wallet's own route resolver would still
resolve a never-attached wallet to clearnet, the CLI narration would still
call it a choice, and the always-on recovery loop had already been forced
to invent the phrase "an unconsented off" for a state the type refused to
name. Fail-closed demands the backstop at the routing decision, which
lives in the wallet; the presenter goes back to being a pure projection.

Consequences: the FFI mode enum grows a variant and its generated bindings
regenerate on both platforms; the CLI status narration distinguishes "never
enabled" from "switched off"; the mobile coordinator's recovery predicate
becomes a plain match on unattached-or-died-or-failure; and a recreated
wallet on a live session reports unattached rather than off, so the second
fail-open path the review identified (a configure re-run on the same
backend instance) closes by construction.

## Amendment (2026-07-28): the price fetch returns to mixnet-only

The 2026-07-27 amendment above, which restored a clearnet default for
the price fetch, is superseded; the 2026-07-23 rule is reinstated in
full. The consumer-convergence audit of 2026-07-28 supplied the
deciding evidence: with routing policy left "entirely in the caller,"
both shipping consumers got it wrong in the same direction. zingo-cli's
price command advertises the mixnet method while calling only the
clearnet one, and zingo-mobile fetches over clearnet while its own
disclaimer tells the user the mixnet covers price-fetch. A per-caller
choice that every caller fumbles identically is not a policy; it is a
leak with extra steps.

The reinstated rule: the price fetch travels only over the mixnet. In a
nym build, the shared session driver refuses the fetch in every Mixnet
Mode state except ready — including switched off, whose consent covers
Transmission and never price, because price contacts a third party
outside the Zcash ecosystem and carries no availability argument. A
build without the nym feature compiles no fetch at all. zingo-pc, which
today ships no nym stack, consequently loses its price display until it
adopts the converged stack; that consequence is accepted deliberately,
as an incentive to converge rather than a cost to engineer around. The
`MixnetPriceFetch` route evidence survives as the fetch's only success
shape.

## Amendment (2026-08-08)

ADR 0040 supersedes the serially gated escalation ratified above: send
escalates as a hedged race of full paths, each arm pairing its own
Destination with its own exit. The consumption model, the mobile
attach boundary, and the fail-closed posture stand.

## Amendment (2026-08-10): the mobile proxy shim moves to zingo-mobile

The UniFFI proxy shim (`zingo-nym-proxy-ffi`), its binding generator
(`uniffi-bindgen`), and the workbench Android bundler leave this
repository. zingo-mobile now hosts the shim source in its own `nym-host`
workspace and generates its own bindings (zingo-mobile PR #1251). The
2026-07-21 rulings stand unchanged: the mobile proxy boundary remains
UniFFI, and the runtime-boundary generalization remains in force; only
the shim's hosting repository changes. This workspace keeps the desktop
`nym-proxy` binary and its `bundle-nym-proxy` workbench tool.

## Amendment (2026-08-26): the switched-off consent covers the price fetch

The 2026-07-28 ruling above, that the switched-off consent covers
Transmission and never the price fetch, is reversed. The deliberate
SwitchedOff now consents to a clearnet price fetch exactly as it consents
to a clearnet send. The mobile sessions supplied the deciding evidence. A
user who declines the mixnet starts every session SwitchedOff, and the
price display never works for that user, in any session, by design. The
refusal was meant to prevent an unconsented leak, but the state it fires
in is the one state the user reached by an explicit choice. The same
session's sends already travel clearnet to an indexer that knows the
wallet's address set, and a price API learns less than that indexer
already holds.

The amended rule: `update_current_price` follows the one route resolver.
Ready races the sources through the tunnel as before. SwitchedOff races
the same sources over untunneled HTTP, and the documentation disclosure
from the 2026-07-27 amendment applies to that route again. The
transitional states (Unattached, Bootstrapping, Died) keep their typed
refusals, so absence is still not consent. The success value attests the
route as a two-variant enum, mixnet with its SOCKS5 endpoint or clearnet,
replacing the tunnel-endpoint string, and `PriceFetchRequiresMixnet`
leaves the error surface as unreachable. `probe_destinations` remains
mixnet-only, since its subject is the mixnet transport itself.

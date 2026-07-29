# 24. Consumers converge on a zingolib-owned mixnet surface

Date: 2026-07-28

Status: draft — ratified in session, pending review

## Context

Three consumers front this wallet: zingo-cli (in-process Rust),
zingo-mobile (a UniFFI cdylib under React Native), and zingo-pc (a neon
Node addon under Electron). A three-tree audit on 2026-07-28 measured
each against ADR 0011's design, which confines consumer difference to
one dimension — how the SOCKS5 endpoint and liveness signal come to
exist (spawn or attach) — and found divergence along six axes instead.

The CLI carries its own startup policy, proxy-path precedence, and
error prose. Mobile mints its own Mixnet Mode wire strings in its FFI
shim, runs a TypeScript lifecycle coordinator with two poll cadences
layered over zingolib's supervisor cadence and the proxy shim's, and
recovers error identity by substring-matching zingolib's prose after
flattening typed errors three times; its iOS build has no mixnet at all
yet leaves the send gate open. zingo-pc has no mixnet wiring and a
stringly JSON boundary that cannot carry typed evidence. Both shipping
consumers fetch price over clearnet while their disclaimers claim the
mixnet covers price-fetch. Mobile's cdylib links two copies of
zingo-netutils, and the copy its own code calls has no SOCKS5
capability compiled in. Mobile pins zingolib by branch, a moving
target.

## Decision

One mint, N renderers. The convergence is a set of ownership rules,
each ratified separately in the 2026-07-28 grilling session:

1. **Wire contract.** zingolib mints the canonical representation of
   Mixnet Mode: the five ratified states with serde/Display strings
   ("unattached", "switched_off", "bootstrapping", "ready", "died")
   and a typed status struct. Mobile's mint site and its TypeScript
   re-declaration are deleted in favor of consuming zingolib's; the
   CLI renders from the same enum; zingo-pc receives it typed.

2. **Session policy.** zingolib owns a session driver: enabling a
   connected session forces the mode on, the driver owns the recovery
   predicate, and one Rust-owned liveness clock replaces the three
   unsynchronized cadences. Status reaches consumers by subscription,
   as the proxy shim's death callback already does. The CLI's
   forced-on block and mobile's MixnetCoordinator retire; a native
   clock also survives mobile's background timer throttling.

3. **Provisioning.** zingolib::nym owns the single proxy-path resolver
   (the four-tier precedence, one standardized ZINGO_NYM_PROXY
   variable, empty-is-absent), parameterized by platform hints. The
   driver accepts a spawn strategy carrying hints or an attach
   strategy carrying an address. zingo-netutils keeps building and
   bundling the binary.

4. **Consent at start.** A startup-time opt-out is the explicit act
   that reaches switched off: the driver takes a consent parameter, so
   a --no-mixnet flag or its UI equivalent records the same clearnet
   consent as an in-session toggle-off. The glossary's Mixnet Mode
   entry says so.

5. **Boundary carrier.** The contract crosses each boundary typed,
   from one Rust source. Mobile's UDL models real UniFFI enums,
   records, and typed errors generated from zingolib's types,
   extending the shim's golden-wire-pin pattern to the wallet
   component. zingo-pc keeps neon but consumes zingolib's serde
   serialization of the same types, pinned by the same golden
   fixtures. Consumers match variants, never prose.

6. **Price.** The 2026-07-23 mixnet-only rule is reinstated in full,
   superseding the 2026-07-27 clearnet restoration; ADR 0011 carries
   the amendment. The driver refuses price in every state except
   ready; a build without the nym feature compiles no fetch.

7. **Dependencies.** Consumers declare exactly one wallet dependency:
   zingolib at a git rev, never a branch. zingolib re-exports the
   netutils items consumers legitimately need, so direct netutils
   dependencies retire — which resolves the double-compile and its
   capability-less duplicate.

8. **Narration.** Rendering stays per-consumer, from the typed state
   and typed errors only. zingolib keeps owning the shared semantic
   sentences: the IP-correlation disclaimer and refusal remedies.

Implementation proceeds zingolib first (on the attach_retune line),
zingo-mobile second (delete the coordinator, the mint, the markers,
and the dead legacy toggle; close the iOS fail-open), zingo-pc third
(typed neon consumption; spawn everywhere except the Mac App Store,
which attaches, discriminated by the already-plumbed process.mas).

## Considered options

A shared consumer-facing crate owning the wire contract was rejected
while zingo-viewmodel remains unmerged; zingolib is the source that
exists. Per-consumer policy engines held to a written spec were
rejected because the audit shows spec-only convergence decaying — the
disclaimers already promised behavior no consumer implemented. UniFFI
for zingo-pc was rejected for now: node bindgen is unproven here and
would rewrite a seventy-function surface for a nym-scoped goal.

## Consequences

zingolib grows a consumer-facing surface (driver, resolver, wire
types, re-exports) and its API stability burden grows with it. Mobile
deletes more code than it gains and loses its only defense — the
app-side consent bit — in exchange for the wallet-level five-state
backstop that ADR 0011's 2026-07-28 amendment already ratified.
zingo-pc's price display goes dark until it converges, deliberately.
The iOS open send gate is a live fail-open until the converged gate
lands there; it warrants an immediate issue independent of this
record. Golden wire fixtures multiply across three repos and must stay
byte-identical; the shim's existing three-language pattern is the
template.

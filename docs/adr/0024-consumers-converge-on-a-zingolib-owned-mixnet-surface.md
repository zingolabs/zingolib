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

## Amendment (2026-08-03): zingo-perspective becomes the funnel dependency

The Considered-options section rejected a shared consumer-facing crate
"while zingo-viewmodel remains unmerged." That condition is now being
dissolved deliberately. The editorial layer is re-extracted on a
fresh branch off dev, so the crate is born conformant to this record
rather than rebased across the mint's relocation; the stale
`view-model` and `viewmodel-crate` branches and PR #2497 serve as an
inventory of what belongs in the editorial layer, never as a rebase
base. The crate is named zingo-perspective, superseding the working
name zingo-viewmodel of the abandoned branches (2026-08-03 naming
ruling): the singular is load-bearing — one house perspective, N
renderers — and "viewmodel" invited an MVVM misreading.

The retarget fires when the extraction's seed slice merges to dev,
not when the last projection moves in. The seed's re-export spine is
built consumer-complete, from an audit of what zingo-cli,
zingo-mobile, and zingo-pc actually import from zingolib, so each
governed consumer repoints exactly once; later slices are additive
behind an already-standing funnel. "Exactly once" counts
dependencies, not edits: the rename is mechanical only for the
mirrored surface, because the editorial types (the value-transfer
and finsight types) leave zingolib entirely and live at the
perspective crate root, so an editorial consumer also rewrites those
import paths and adds the extension-trait imports at its editorial
call sites. From that merge, rule 7 reads:
consumers declare exactly one wallet dependency, zingo-perspective at
a git rev, never a branch. zingo-perspective re-exports the wire
mint, the session driver's consumer surface, and whatever zingolib
items consumers legitimately need, exactly as zingolib re-exports
netutils items today; it re-exports and projects, it never redefines.
Rules 1 through 6 and 8 are untouched: zingolib remains the mint and
the policy owner, the perspective crate owns only the editorial
projection, and consumers own only rendering. The side-by-side
alternative — consumers holding both a zingolib and a
zingo-perspective dependency — was rejected because two pins can
drift, and drift recreates the netutils failure mode one layer up:
duplicate compiles of shared types, and golden fixtures silently
pinning different revisions of the same wire.

The retargeted rule remains what the original was: a contract over the
consumers this project governs, not a property the compiler enforces.
Every item the perspective crate re-exports must stay public in
zingolib, so a direct zingolib dependency remains possible for anyone.
Enforcement lives in the governed consumers' manifests and CI — the
golden wire fixtures and the reference consumer's canary build — where
a violation is a reviewable diff. The privacy ratchet remains the
instrument that shrinks the bypassable surface, ratcheting zingolib's
public API down toward what the perspective crate actually consumes.
The Reference Consumer's ADR 0028 exemption carries over unchanged: it
tracks workspace HEAD by path, now through zingo-perspective. Until
the re-extraction merges, rule 7 stands as originally written.

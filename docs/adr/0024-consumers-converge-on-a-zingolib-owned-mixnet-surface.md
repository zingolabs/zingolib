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

## Amendment (2026-08-04): the editorial layer is the `perspective` module, not a crate

The editorial layer — the value-transfer derivation, the finsight
rollups, and their types — was re-extracted into a workspace crate,
zingo-perspective, on the branch `viewmodel_seed` (draft PR #2611,
with the mixnet projection slice stacked as PR #2617). That extraction
carried its own amendment to this record, retargeting rule 7 so that
governed consumers would declare zingo-perspective as their one wallet
dependency, reached through a funnel of path-for-path re-exports. This
amendment declines the extraction before it merges. Both pull requests
close as superseded, and `viewmodel_seed` joins `viewmodel-crate` as
an inventory of what belongs in the editorial layer, mined but never
rebased.

The editorial layer instead becomes the `zingolib::perspective`
module, compiled behind a `perspective` cargo feature that is the
crate's first default-on feature. Rule 7 therefore stands as
originally written: consumers declare exactly one wallet dependency,
and it is zingolib. The deciding ground is churn. The funnel form
required every governed consumer, across three repositories, to
repoint its manifest and rename every import, and required the funnel
to pin a re-exported surface path-for-path forever after; the module
form requires none of it, because consumers already hold the one
dependency the funnel existed to provide, and because the derivation
returns to inherent `impl LightWallet` and `impl LightClient` blocks,
so no call site changes and no trait import appears. The extraction's
naming ruling survives the placement reversal: the layer is named
perspective, the singular load-bearing — one house perspective, N
renderers — with viewmodel rejected for inviting an MVVM misreading.

What the module surrenders is recorded plainly. The crate boundary is
the only construct in Rust that denies a crate's own code access its
consumers have: visibility only narrows outward, so every `pub` item
a consumer can import is nameable from every module of the defining
crate, and the extraction's editorial-independence guarantee held by
construction where the module's holds by convention. The resulting
boundary is asymmetric. In the core-to-editorial direction the proof
is structural and already standing: CI's cargo-hack feature-powerset
job compiles the perspective-off subset of every feature combination
under `-D warnings` on every pull request, so zingolib building
without its editorial layer is a permanent compiler-checked fact. In
the editorial-to-core direction there is no mechanism at all — no
import allowlist, no workbench gate — by explicit ruling: developer
discipline and review carry it. The five summary primitives the
extraction had promoted to `pub` remain `pub(crate)`, read by the
module as any sibling reads them, and the roughly 170 editorial items
the extraction would have removed from zingolib's public API remain
public, since consumers must import them from somewhere.

The editorial items move clean: the editorial half of
`wallet::summary` relocates to `zingolib::perspective` with no aliases
or deprecated re-exports left at the old paths, so governed consumers
rewrite the `use` lines naming the moved types once, and nothing else.
The canonical `TransactionSummary` stays where it is. The golden
fixtures captured from the pre-extraction implementation port
unchanged and keep pinning the same contract. The mixnet projection
remains deliberately absent, to be re-derived from the zingolib-owned
typed status of rules 1 and 2 in a later slice that lands in the
module. The Reference Consumer's placement under ADR 0028 is
untouched, its path dependency reading zingolib directly; the
unmerged wording that routed it through zingo-perspective reverts
when that record lands.

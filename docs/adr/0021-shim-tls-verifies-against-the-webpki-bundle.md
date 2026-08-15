# The mixnet shim's TLS verifies against the compiled-in webpki bundle

Status: accepted (zingolib#2531); amended 2026-08-10 after zingolib#2666.
Scoped to the mobile mixnet shim's own
TLS — the `zingo-netutils` standalone workspace, where the nym SDK's HTTP
clients reach the Nym directory, API, and gateways. Wallet↔indexer TLS and
every other TLS surface in the parent workspace are unaffected.

The `zingo-netutils` workspace replaces `rustls-platform-verifier` through
`[patch.crates-io]` with a workspace-local crate
(`webpki-verifier-shim/`, published name and version mirroring upstream)
that verifies server certificates against the compiled-in Mozilla root
bundle (`webpki-root-certs`), identically on every platform. The stand-in
implements exactly the surface its one consumer uses — reqwest's
`Verifier::new` and `Verifier::new_with_extra_roots`, installed as a
rustls `ServerCertVerifier` — and nothing more, so any future consumer of
a wider upstream surface fails at compile time instead of silently
diverging.

## Why

On Android, `rustls-platform-verifier` requires JVM-side initialization
with an app `Context` before any handshake, plus a Kotlin component in the
consuming app. The shim cannot provide that within its ratified
constraints: acquiring the `JavaVM` and `Context` demands hand-written JNI
— `unsafe` code the shim's `#![forbid(unsafe_code)]` invariant (the ADR
0011 mobile amendment) exists to exclude — and every alternative shape
exports per-app ceremony across the shim boundary that ADR 0011 defines as
"load the library, talk SOCKS5." The failure mode this caused in the field
is documented in zingolib#2531: every mixnet enable on Android failed at
the first TLS handshake, which zingo-mobile's silent alpha surfaced as a
wallet stuck in `off`.

The compiled-in bundle was chosen over the operating system's trust store
deliberately, not merely expediently. It keeps the workspace free of
hand-written unsafe, and it makes certificate verification byte-identical
on Android, iOS, and every host platform — one behavior to reason about,
test in plain CI, and debug during the alpha, instead of one per vendor
fleet. The prices are real and accepted: the bundle ages between re-pins,
enterprise and user-installed CAs are never honored, and the platform's
own revocation machinery is bypassed. For this surface those prices are
small — the shim talks only to Nym infrastructure, whose certificates
chain to public Mozilla-included roots.

## Update policy

The bundle must never age silently. Three mechanisms, in increasing
urgency:

1. **Release tracking (mandatory):** automated dependency PRs (Dependabot
   or equivalent) watch `webpki-root-certs`, so every upstream release of
   the packaged Mozilla store proposes a re-pin. Merging these is routine
   maintenance, expected within days.
2. **Staleness alarm (mandatory):** a scheduled nightly workbench check
   runs `cargo update -p webpki-root-certs --dry-run` in the
   `zingo-netutils` workspace and fails when a newer release exists, so a
   missed or disabled release-tracking bot cannot let the bundle rot
   unnoticed.
3. **Distrust alarm (the reason lag matters):** the same nightly job
   watches Mozilla's canonical source — the content hash of
   `certdata.txt` in mozilla-central, or the CCADB included-roots report —
   and opens an issue on any change, catching emergency root distrusts in
   the window before they reach a crate release.

The patch itself is re-validated on every nym-sdk or reqwest bump: the
stand-in compiles against exactly the surface reqwest uses, so a widened
requirement surfaces as a compile failure in the shim build, and the
resolution is to extend the stand-in's parity deliberately, never to drop
the patch by accident.

## Alternatives, and when to reconsider

Platform-verifier integration was rejected in two shapes: shim
self-initialization via `JNI_OnLoad` (bounded unsafe in a leaf crate,
plus an `ActivityThread` reflection with no API contract), and an
exported init entry point (the same unsafe, plus a mandatory app-side
call crossing the ADR 0011 boundary). Both were rejected for the same two
properties this decision optimizes away: hand-written unsafe, and
per-platform behavioral divergence. The full trade-off record is
zingolib#2531.

This decision stands unless its premises move. Reconsider — via a
superseding ADR, not an edit — if upstream `rustls-platform-verifier`
gains an initialization path requiring neither hand-written unsafe nor
per-app ceremony; if the shim ever needs enterprise or user-installed CA
trust; or if the Nym infrastructure's certificate chains stop resolving
against the Mozilla bundle. The nightly upstream watch in the update
policy is also the tripwire for the first of these.

## Amendment (2026-08-10): the decision outlives the shim's departure

zingolib#2666 moved the mobile UniFFI proxy shim to zingo-mobile's
`nym-host` workspace, so this workspace no longer builds a mobile
artifact. The scope named above therefore reads today as the
`zingo-netutils` standalone workspace itself: the nym SDK's HTTP clients
behind the desktop `nym-proxy` binary and the crate's library consumers.

The decision stands on its surviving rationale, ratified 2026-08-10. The
compiled-in bundle keeps certificate verification byte-identical on every
host that builds this workspace, keeps the workspace free of hand-written
unsafe, and stays testable in plain CI. The Android initialization story
in "Why" is the decision's origin, not its continuing justification here.
The relocated shim consumes this crate as a git dependency from its own
standalone workspace, and a `[patch.crates-io]` substitution never
crosses a workspace root, so the verifier the shim's TLS resolves is
zingo-mobile's to declare and to record. The update policy and the
reconsideration tripwires above are unchanged.

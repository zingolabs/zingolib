# Mobile adoption of the Nym transmission arc: the critical path

Recorded 2026-07-21. Tracked as GitHub issues: #2508 (tracking) over the
chain #2502 (CP-1), #2503 (CP-2), #2504 (CP-3), #2505 (CP-4), #2506
(CP-5), #2507 (CP-6) — CP-1 and CP-2 are parallel roots, each later step
blocked by its predecessor.

This file documents ONLY the critical path from the
current state of PR #2470 to the end state: a user on an iOS device and an
Android device sends ZEC over the Nym mixnet, fail-closed, with witness
rotation — exactly what the desktop smoke ladder proved live on
2026-07-21. The design decisions behind this sequence live in ADR 0011
(the 2026-07-21 amendments: the fourth mode, the lifetime coupling, and
"the runtime boundary generalizes for mobile"); the full step catalog with
its side branches lives in the adoption issue. Everything here is the
serial spine plus the two items that gate shipping.

## The spine

**CP-1 — Merge PR #2470 into feat/ironwood.** Every ratified gate is met:
the three-stage live smoke ladder is complete (a real send over the
mixnet on the first random witness), readiness is health-gated, indexer
connections are https-only, and the PR description opens with the
hand-test procedure. Remaining mechanics: push the rewritten branch,
un-draft, request review. Feasibility: high. Difficulty: small; the
timing is the reviewers'. Development downstream does not wait for this —
zingo-mobile can pin a branch rev — but shipping does.

**CP-2 — The attach seam in zingolib (the sole zls code change).**
`LightClient::attach_mixnet(socks5_addr)` beside the spawn path: readiness
gated on a data round trip (the increment-17 lesson — never a bare TCP
connect), liveness thereafter by a periodic endpoint probe whose failure
lands `Died` and clears the address. Falsifiers: probe-failure lands died;
an attached session never routes clearnet without the deliberate off;
status renders identically for spawned and attached transports.
Feasibility: certain — the supervisor variants, health gate, and Died
semantics all exist to pattern-match. Difficulty: small, one to two days.
Startable immediately, in parallel with CP-1.

**CP-3 — The iOS transport (the long pole).** The standalone
zingo-netutils workspace — already its own resolution unit with its own
lockfile — gains a `cdylib` target exposing three C functions over
`NymProxy`: start (yields the local SOCKS5 address or an error), stop,
and a death callback. Packaged as an XCFramework; the app hosts it and
hands the address to `attach_mixnet`. Feasibility: proven in principle —
NymVPN's shipping iOS client embeds the same Rust nym core via FFI, and
this needs strictly less (a SOCKS5 client only; no tunnel APIs, no VPN
entitlement, ordinary App Store networking). The risks are build-system:
cross-compiling the TLS stack (aws-lc-rs / ring) for device and
simulator, and hosting a tokio runtime in a loaded dylib. Difficulty:
the hardest single step — one to three weeks of build and integration
engineering. Depends on CP-2 for the wallet-side attach.

**CP-4 — The wallet-layer bindings in zingo-mobile.** Five methods
(`attach_mixnet`, `enable_mixnet`, `disable_mixnet`, `mixnet_mode`,
`mixnet_bootstrap_detail`) and the four-variant mode enum added to the
binding layer. All plain strings, options, and one enum — expressible in
a UniFFI UDL or the older command bridge alike. The proxy's C ABI never
appears here: bindings carry the wallet layer only. Feasibility: certain.
Difficulty: small, days including regenerated bindings and TypeScript
types.

**CP-5 — App-layer policy and UX.** Forced-on for connected sessions,
per-session off as informed consent, bootstrap narration from the detail
string, died prompting re-enable, refusals surfaced verbatim.
Feasibility: certain; ordinary app work over an API shaped for it.
Difficulty: moderate — one to two weeks, dominated by UX decisions and
review cycles rather than code.

**CP-6 — Device smoke on both platforms.** The PR's hand-test procedure
on hardware: status until ready, a price fetch as the no-spend tunnel
proof, a small self-send. Feasibility: high — the identical ladder just
passed on desktop. Difficulty: small as engineering; budget days of
test-and-fix for what only devices surface (cellular bootstrap latency,
backgrounding mid-send, died recovery).

## Gating but off the spine

The Android transport runs in parallel with CP-3 and finishes earlier
(moderate, about a week; either exec-from-nativeLibraryDir with
extractNativeLibs=true, or the same C-ABI dylib model as iOS — the
uniform model is preferred), so iOS dominates the schedule. Shipping —
as distinct from development — additionally requires the zingolib pin
bump in zingo-mobile after CP-1, and inherits feat/ironwood's own merge
timeline, which is outside this arc's control.

## The estimate

With one engineer plus agent support: roughly four to seven weeks from
CP-1 to CP-6, dominated by the iOS build engineering (CP-3) and app
review cycles (CP-5).

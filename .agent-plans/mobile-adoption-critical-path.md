# Mobile adoption of the Nym transmission arc: the critical path

Recorded 2026-07-21. Tracked as GitHub issues: #2508 (tracking) over a
seven-step chain — #2502 (step 1, merge), #2503 (step 2, attach seam,
DONE), #2513 (step 3, Android transport), #2505 (step 4, wallet
bindings), #2506 (step 5, app policy/UX), #2507 (step 6, Android device
smoke), #2504 (step 7, the Mac-gated iOS block, last).

## REORDERED Android-first (user directive, 2026-07-21)

The iOS work needs a Mac; everything Android needs builds on Linux. The
critical path therefore completes on Android first, and the entire iOS
block — cross-compile, XCFramework, Swift hosting, iOS smoke — runs
last as step 7. Mapping from the original spine below: CP-1/CP-2 keep
their places (steps 1-2); the Android transport, formerly off-spine,
is promoted to step 3 (#2513) using the uniform UniFFI shim model (the
committed, host-proven `nym-proxy-ffi` crate, cross-compiled with
cargo-ndk, loaded from jniLibs — exec-from-nativeLibraryDir stays the
fallback); CP-4/CP-5 become steps 4-5 scoped Android-first; CP-6
becomes step 6, the Android smoke that completes the path; CP-3 (iOS)
becomes step 7. The spine prose below keeps its original CP numbering
for the trail; the tracker #2508 is the order of record.

## Lane coordination (2026-07-21, post-reorder)

Two agents, no file overlap:
- The CP-3/shim agent (netutils workspace): step 3's build side —
  rustup Android targets, cargo-ndk cross-compile of `nym-proxy-ffi`
  for the Android ABIs (aws-lc-rs + the ratified transitive ring under
  NDK), uniffi-bindgen Kotlin generation, and the death-observer wiring
  noted in the slice-1 gaps. Host facts, probed 2026-07-21: cargo-ndk
  is installed, NDK 28.2.13676358 is at ~/Android/Sdk/ndk, no Android
  rust targets installed yet, ANDROID_NDK_HOME unset.
- The CP-2/attach agent (this note's author): step 4 in the
  zingo-mobile checkout at ~/src/zingolabs/zmobs/dev on a NEW branch —
  the five UDL functions + wrappers + `features = ["nym"]` on the
  zingolib dep + TS types — GATED on the user pushing
  `nym_mobile_adoption` to zingolabs so the seam is pinnable; then step
  3's zmobs side (jniLibs packaging, Kotlin glue) once the shim's
  Kotlin bindings exist.

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
lockfile — gains a `cdylib`/`staticlib` UniFFI shim member crate exposing
the proxy over `NymProxy`: start (yields the local SOCKS5 address or an
error), stop, and a death callback (a UniFFI callback interface the host
implements). Packaged as an XCFramework; the app hosts it and hands the
address to `attach_mixnet`. AMENDED 2026-07-21 (was: a raw hand-written
C ABI): the boundary is a **UniFFI surface**, not hand-written C. Reason,
empirically verified this session: `#![forbid(unsafe_code)]` does NOT fire
on uniffi's proc-macro-generated FFI unsafe (a control crate with forbid
compiled a `#[uniffi::export]` whose expansion carries 57 unsafe
occurrences, while a single hand-written `unsafe` block in the same crate
DID trip forbid). So UniFFI is the ONLY option that keeps every file's
`#![forbid(unsafe_code)]` intact with NO exception — zero hand-written
unsafe, and forbid still guards against any a contributor later adds. A
raw C ABI would force a ratified forbid exception and hand-rolled unsafe.
The plan's earlier "two boundaries never meet" concern is unaffected: two
separate UniFFI components (wallet, proxy) are distinct surfaces with
distinct generated bindings. Feasibility: proven in principle — NymVPN's
shipping iOS client embeds the same Rust nym core via FFI, and this needs
strictly less (a SOCKS5 client only; no tunnel APIs, no VPN entitlement,
ordinary App Store networking). The risks are build-system:
cross-compiling the aws-lc-rs TLS backend (nym-sdk also transitively links
ring via its cosmos core — a ratified exception, ADR 0011, cross-compiled
the same) for device and simulator, and hosting a tokio runtime in a loaded
dylib. Difficulty:
the hardest single step — one to three weeks of build and integration
engineering. Depends on CP-2 for the wallet-side attach.

**CP-4 — The wallet-layer bindings in zingo-mobile.** Five methods
(`attach_mixnet`, `enable_mixnet`, `disable_mixnet`, `mixnet_mode`,
`mixnet_bootstrap_detail`) and the four-variant mode enum added to the
binding layer. All plain strings, options, and one enum — expressible in
a UniFFI UDL or the older command bridge alike. The proxy's UniFFI
component (CP-3) stays a distinct surface: these bindings carry the
wallet layer only. Feasibility: certain.
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
extractNativeLibs=true, or the same UniFFI dylib model as iOS — the
uniform model is preferred), so iOS dominates the schedule. Shipping —
as distinct from development — additionally requires the zingolib pin
bump in zingo-mobile after CP-1, and inherits feat/ironwood's own merge
timeline, which is outside this arc's control.

## The estimate

With one engineer plus agent support: roughly four to seven weeks from
CP-1 to CP-6, dominated by the iOS build engineering (CP-3) and app
review cycles (CP-5).

## CP-3 (#2504) working notes — claim + gating decision (2026-07-21)

CLAIMED by this agent, in parallel with CP-2/#2503 (another agent, the
zingolib attach seam). No file overlap: CP-3 works ONLY the standalone
zingo-netutils workspace (own lockfile); CP-2 works zingolib/src/nym.
The "blocked by #2503" edge is runtime integration (the app wires the
address into attach_mixnet), NOT a compile dependency — the cdylib
consumes only NymProxy, which exists.

Prospective file claims (gated on the decision below):
- `zingo-netutils/` workspace: a NEW FFI shim member crate exposing the
  three C functions (start/stop/death-callback) over NymProxy.
- `tools/workbench/` : an XCFramework packaging tool (Rust std; runs on
  macOS).
- iOS Rust targets / cargo config for aarch64-apple-ios + sim.
- this file.

SLICE 1 DONE (2026-07-21): the UniFFI shim member crate
`zingo-netutils/nym-proxy-ffi` (`zingo-nym-proxy-ffi`) exists and builds on
the HOST as both a cdylib (libzingo_nym_proxy_ffi.so) and a staticlib,
proving the two build risks the plan named — the UniFFI FFI shape as a
loaded dylib, and hosting a tokio runtime inside it. Surface: a
`MixnetProxyHandle` uniffi::Object wrapping a multi-thread Runtime + an
`Option<NymProxy>`, with `start()` (constructor -> handle or ProxyFfiError),
`socks5_address()`, `stop()` (idempotent), and a `ProxyDeathObserver`
callback interface. `#![forbid(unsafe_code)]` KEPT and intact — zero
hand-written unsafe, the whole reason for the UniFFI amendment. uniffi 0.28
and nym-sdk coexist in the standalone lockfile; clippy -D warnings clean;
main workspace + netutils default unaffected (parent excludes netutils).

SLICE-1 KNOWN GAPS (follow-on, none blocking the build proof):
- ProxyDeathObserver is defined but NOT yet wired: NymProxy exposes no
  death signal, and start() takes no observer yet. Death detection ties to
  the Q4 liveness decision — likely the shim monitors its nym client and/or
  zingolib's attach probe covers it. Wire when CP-2's attach lands.
- start() returns the raw address with no in-shim health gate/redraw yet;
  CP-2's attach_mixnet health-gates (increment-17 round trip) on the wallet
  side. If we want the proxy-owner-remediates redraw (Q3) in the shim,
  extract the binary's health_gate+reconnect loop into a shared lib fn both
  bin/nym-proxy.rs and this shim call (DRY).
- MACOS-ONLY REMAINDER (cannot run on this Linux host): rustup targets
  aarch64-apple-ios + sim, cross-compiling the aws-lc-rs TLS backend (and
  nym-sdk's transitive ring — the ratified exception below),
  uniffi-bindgen for the Swift bindings, and the XCFramework packaging
  (a workbench tool). This is the bulk of CP-3's 1-3 week estimate.

RING EXCEPTION (user-ratified 2026-07-21, decision A): the backend policy is
aws-lc-rs, never ring, but nym-sdk transitively links ring via its cosmos
core (unseverable without a barred fork/[patch]) and the main wallet already
carries a build-time ring via zcash_proofs's param downloader. Both accepted
as unavoidable transitive exceptions, documented in ADR 0011, deny.toml, and
netutils/Cargo.toml; upstream removal filed as a tracked follow-up (decision
B queued). The nym ring is confined to this standalone build; the wallet lock
stays ring-free at runtime.

GATING DECISION (RESOLVED — user ratified UniFFI 2026-07-21): a hand-written C ABI —
`extern "C"` bodies, raw-pointer marshalling, the death-callback function
pointer, CString/Box `into_raw`/`from_raw` — REQUIRES `unsafe`, which
collides head-on with the hard rule that every Rust file carries
`#![forbid(unsafe_code)]` (forbid, not deny, so uncircumventable in-file).
Recommended resolution: confine ALL unsafe to ONE purpose-built FFI shim
crate that carries a scoped exception, leaving every other crate's forbid
intact — mirroring the ratified nym-deps exception. Awaiting ratification
before writing the C boundary. On THIS Linux host the high-value,
locally-provable first slice is a host-target build of that cdylib
proving the two risks the plan names (the C ABI marshalling and hosting a
tokio runtime in a loaded dylib); the actual aarch64-apple-ios
cross-compile + XCFramework needs macOS.

# Claim: Nym mixnet IP obfuscation (grilling session, 2026-07-15)

**STATUS: DESIGN COMPLETE (2026-07-15).** All 15 questions resolved.
Capstone `docs/adr/0011-nym-mixnet-transmission.md` written; glossary
terms (Nym mixnet, NymVPN, Mixnet Mode, Witness Rotation, Broadcast
Indexer) added to `zingolib/CONTEXT.md` and the stale Tor entry
corrected. No production code written yet — implementation is the next
phase, seam-B first (separate mixnet transmit/price component off a
shared NymProxy, injectable RNG + transport). Base is #2419 ironwood +
#2464's 8 commits (168ee9bb4), cargo check green.

Session anchored in the `reboot_nym` worktree (moved from `dev`, where
an earlier copy of this file briefly lived). Goal: a ratified plan to
route the client's outbound network traffic over the Nym mixnet so
that no server-side attacker learns the client IP. No code edits until
the plan is ratified; decisions crystallise into `zingolib/CONTEXT.md`
and a new ADR as the session proceeds.

**Base:** the Nym work stacks on **#2419 (feat: ironwood support,
b5a1b739e) with #2464's 8 fix commits replayed on top** — rebased
2026-07-15 at the user's direction ("Pick up the 2464 fix commits and
rebase on top of them on top of 2419"). History: reboot_nym was first
fast-forwarded onto #2464 head (eec0bbdb5), then those 8 commits
(typed-Result CLI migration incl. do_info deletion, pepper-sync
scan-range sync completion, ADR-0006 pointer) were `git rebase`d onto
ironwood — a clean replay, no conflicts, re-hashed 168ee9bb4..34fa8be5b.
Compile verification: **GREEN** — `cargo check --workspace
--all-targets` passed (exit 0, 34s) on the rebased tree, so the
do_info-deletion / typed-Result migration reconciled with ironwood's
71 commits with no unbuilt caller. Pre-rebase restore point:
eec0bbdb5.

## Threat model (user-stated, 2026-07-15)

Prevent a server-side attacker from learning the client IP address.
The attacker sits at any service the client contacts; the mixnet exit
gateway seeing the *destination* is acceptable, since services see
only the gateway's IP.

**The indexer is the named adversary.** A send is the transaction
broadcast *to the indexer* (`send_transaction`), so mixnet exists to
hide the client IP from the indexer at broadcast time. Corollary
(resolves the Q8 tension): bare-clearnet sync to that same indexer
logs the real IP against the wallet's address set, letting the indexer
re-link a mixnet-broadcast tx to the client regardless — so bare
clearnet is the explicit "don't care" tier, and **NymVPN-sync +
mixnet-send is the coherent fully-protected posture.** This is why the
NymVPN layer earns its place (Q6, now under sub-agent analysis).

## Decisions ratified so far (2026-07-15)

1. **Transport = Nym; Tor rejected** ("Proceed with the Nym based
   send, forget TOR"), decided against the full Tor-vs-Nym comparison
   table. This carves a scoped exception out of the 2026-07-13
   no-new-deps rule: the nym stack (nym-sdk, nym-http-api-client,
   nym-validator-client, tokio-socks, tower) may enter the
   in-workspace zingo-netutils behind an off-by-default `nym`
   feature. No `[patch]` tables or branch pins.

2. **Per-surface transport tiers** (supersedes the earlier
   all-or-nothing model). Obfuscation is a property of each surface,
   not the whole client; two transports run at once:
   - **Send** → Nym mixnet, required, fail-closed.
   - **Price-fetch** → Nym mixnet, required, fail-closed.
   - **Sync** (indexer queries + mempool stream) → NOT required to be
     mixnet; clearnet OR NymVPN acceptable. Sync degrades gracefully;
     it does not fail closed.

3. **Live user toggle for the Nym mixnet** — the mixnet transport can
   be turned on and off at runtime, not only at startup. Consequence:
   the seam holds a *mutable* shared transport-state, not a
   construction-time-fixed connector; toggling on incurs bootstrap
   latency (POC ~seconds-to-120s) during which the mixnet-only
   surfaces are unavailable-but-connecting.

### Glossary split (must not blur)

- **Nym mixnet**: 5-hop Sphinx network via nym-sdk's embedded SOCKS5
  client. Max anonymity, high latency. Carries send + price-fetch.
- **NymVPN**: Nym's VPN product. "Fast" = 2-hop AmneziaWG
  (WireGuard); "Anonymous" = 5-hop mixnet. Lower latency than the raw
  mixnet SDK. An acceptable sync tier — but **user-provided at the OS
  level, NOT embedded** (Q6 resolved, see below).

## Network-surface audit (2026-07-15, this worktree)

Production outbound surfaces — the complete list:

1. Indexer gRPC (`zingo-netutils::GrpcIndexer`): all RPCs including
   send_transaction and pepper-sync's long-lived mempool stream.
   Default `zec.rocks:443`.
2. Server-select probe fan-out (`zingo-cli/src/server_select.rs`):
   concurrent get_lightd_info to all ~14 URIs in
   `most_up_indexer_uris.rs` when no `--server` is given. Loudest
   leak under the threat model.
3. Price fetch (`zingo-price/src/lib.rs:205`): reqwest GET to
   api.gemini.com, clearnet since the Tor removal.

Everything else socket-touching is test-only. The "valar group
broadcast service" is **hypothetical as of 2026-07-15** (user
confirmed) — a not-yet-built future broadcast surface. It gets a
reserved seat behind the routing seam and blocks nothing.

## Findings already established

- Send has never run over Tor (Tor was price-fetch only; removed in
  4d7f03b64, PR #1833, 60eda7090/16460e72d, June 2026).
- `zingo-cli/src/commands.rs` `updatecurrentprice` help text still
  advertises the removed `--tor` flag (stale; fix rides along).
- Send over Nym ran in the unmerged May-2026 POC: zls branch
  `nym_wallet_poc_2_2` + zingo-common branch `nym_wallet_poc_2_1`
  (netutils `nym` feature, `NymProxy` wrapping nym-sdk's
  Socks5MixnetClient, per-RPC `nym: bool`; send on in dd840cc09,
  all-RPCs in 8396e1cad, off in 00c27757a). Merged residue: PR
  #2341's Indexer-trait generalization. POC costs: 5 new direct
  deps, 30s get_info timeout, 120s lifecycle cap, gateway-discovery
  retries.
- zingo-netutils now lives in-workspace (7468d7da0), so the POC's
  `nym_proxy.rs` ports directly — no external pin.
- reqwest (already a workspace dep) supports SOCKS5 proxying, so the
  price fetch can ride the same NymProxy.

## Open questions (grilled one at a time)

- Q3 RESOLVED: valar is hypothetical; reserved seat, blocks nothing.
- Fail-closed (ratified): when the mixnet is ON, a transport failure
  mid-send REFUSES — it never silently drops to clearnet. Clearnet is
  reachable ONLY by the user's deliberate toggle-off (informed
  consent). This is the invariant separating consent from footgun.
- Q4 SUPERSEDED then RESOLVED: price oracle rejected as a DEPENDENCY
  ("we can't depend on the oracle"); price-fetch is obfuscated over
  Nym (reqwest speaks SOCKS5, reuses the mixnet NymProxy). The
  all-or-nothing framing is dead — replaced by Decision 2's per-surface
  tiers. Oracle survives as a someday-maybe, depended on by nothing.
- Q5 RESOLVED (Decision 3): live toggle, not startup-fixed.
- Q6 RESOLVED (2026-07-15, via sub-agent analysis): **NymVPN is
  user-provided at the OS level, NOT embedded.** Our code builds only
  the embedded mixnet SOCKS5 (send + price); sync uses the plain
  connector, which the user may route through a system-installed
  NymVPN app transparently to us. The in-process embed
  (`nym-vpn-core`/`nym-vpn-lib`) is rejected on THREE independent
  grounds: (1) it is **GPL-3.0**, and every zingolib crate is MIT —
  static-linking relicenses the distributed wallet; (2) those crates
  are **not on crates.io** (git-pin only — violates the no-pins rule);
  (3) **OS-impractical** — Android allows one system VPN via a
  consent-gated `VpnService`, and iOS requires a separate entitled
  `NEPacketTunnelProvider` target, so a wallet-FFI process cannot host
  the tunnel. Option 4 (reuse nym-sdk for a 2-hop tier) is impossible:
  the mixnet is fixed 5-hop; 2-hop is NymVPN-only and GPL. Reserved
  follow-up (Option 2, desktop-only): a thin control client speaking
  `nym-vpn-proto` gRPC to a user-installed `nym-vpnd` (no GPL linkage,
  small proto exception) IF we later want to fail-closed-enforce the
  tunnel on desktop before sync. NEW FACT: NymVPN's fast (2-hop)
  gateways require a **paid zk-nym credential**; the plain mixnet SDK
  is free for now — so mixnet send/price cost nothing, but a
  NymVPN-protected sync costs the user money.
- Q7 RESOLVED (2026-07-15): **clearnet send is PERMITTED via the
  user's deliberate toggle-off; mixnet is the DEFAULT.** (User typed
  "7a" but the prose "allow clearnet send" is the 7b branch — intent
  recorded, label disregarded.) Send and price-fetch behave
  uniformly: mixnet by default, clearnet only under explicit
  toggle-off. The fail-closed invariant is unchanged: mixnet-on +
  transport failure = refuse; clearnet iff the user chose it.
- Q8 (OPEN): does bare-clearnet sync (leaking real IP + address set to
  the indexer, partially defeating send-over-mixnet against that same
  indexer) meet the threat model, or is NymVPN sync the intended
  fully-protected posture? Surfaced to user.
- Q8 RESOLVED (2026-07-15): **forced-on-at-startup; the off state is
  NEVER persisted.** Mixnet is on at every launch; toggle-off is
  per-session only. Fail-safe: the worst case is re-disabling each
  session, never a forgotten-off clearnet broadcast. Consequence:
  startup has a "mixnet bootstrapping" window during which send +
  price are unavailable-but-connecting; sync proceeds on its own tier
  meanwhile.
- MOOT (dissolved by the tiered model): the old "mempool-stream
  reconnection over mixnet" concern. Sync — including the long-lived
  mempool stream — rides the SYNC tier (clearnet/NymVPN), NOT the
  mixnet. Only send + price go over mixnet, and both are one-shot. So
  there is no long-lived-stream-over-mixnet fail-closed problem; the
  mempool stream degrades gracefully on its own tier.
- Q9 RESOLVED (2026-07-15): **seam = option B.** GrpcIndexer stays
  plain-only (sync untouched, byte-for-byte); a SEPARATE mixnet
  transmit/price component, built from the shared toggleable NymProxy,
  owns the two mixnet surfaces. Send builds a mixnet-routed client at
  transmit time; price uses reqwest+SOCKS5; both gated on the NymProxy
  being up. Routing is by each operation's static TIER, never a
  per-RPC `nym: bool`.
- Send broadcast strategy (RATIFIED 2026-07-15, FINAL): **ONE indexer
  per send, randomly picked from a curated ~10 reliable low-latency
  list, over the mixnet. The same query is NEVER fired redundantly to
  multiple indexers.** Purpose is witness rotation for privacy — no
  single indexer accumulates a picture of all the user's sends,
  because which one carries any given send is random. The broadcast
  target is decoupled from the sync indexer. (Three-message user
  correction: "No.. not redundant"; "a randomized pick per send";
  "Change the design... don't fire the same query redundantly.")
- Q10 RESOLVED (user-confirmed 2026-07-15): single-pick => ordinary
  single-submission success (plus the existing 852537e09
  duplicate-in-mempool = success rule). No N-way quorum. **On a failed
  submission the send draws a NEW random indexer and retries**
  ("Send is submitted against a random indexer. If it fails another
  random sample picks a new indexer to retry.") — sequential failover,
  never parallel/redundant. Implementation guards (bake in unless told
  otherwise): bound the retry count so an invalid tx can't walk all
  ~10; short-circuit on a substantive rejection (invalid tx will
  reject everywhere) and surface that reason rather than exhausting
  the list.
- Q11 RESOLVED (2026-07-15): **separate curated broadcast list,
  distinct from the sync list** (`most_up_indexer_uris.rs`). Broadcast
  wants reliable tx-relay; sync-ranking wants low get_info latency —
  different criteria, so a change to one must not reshape the other.
  New file, e.g. `broadcast_indexers.rs`.
- Q12 RESOLVED (2026-07-15): **ONE persistent mixnet client** (the
  NymProxy, bootstrapped once at startup) — a fresh client per send is
  unneeded for the indexer threat (the indexer never sees the Nym
  client identity over the mixnet) and too costly (per-client gateway
  registration). BUT **fresh SURBs/circuits per send WITHIN that one
  client**, as cheap defense-in-depth against a network-level observer
  correlating sends by reply path. IMPLEMENTATION FLAG: Nym's model is
  per-Sphinx-packet stratified routing + single-use reply blocks, not
  Tor circuits — verify against nym-sdk what per-send reply/connection
  isolation it actually exposes; record the real mechanism, don't
  overpromise "circuits".
- Q13 RESOLVED (2026-07-15): **mixnet bootstrap is scoped to
  CONNECTED sessions; skipped under `--offline`.** Offline sessions
  never transmit, so they pay no mixnet bootstrap cost; connected
  sessions bootstrap eagerly at startup (per Q8) so send is ready
  with no surprise 120s wait. Net rule: mixnet is forced-on whenever
  the session can transmit at all, absent when it can't.
- Q14 RESOLVED (2026-07-15): **mocked logic in CI, opt-in live smoke
  test by hand.** All send/toggle/fail-closed logic is tested in CI
  against an injected mock transport + a seeded/injectable RNG
  (assert: random pick from the broadcast list, sequential failover on
  unreachable, substantive-rejection terminal, duplicate=success,
  mixnet-down fails closed, toggle flips availability, --offline skips
  bootstrap). The real NymProxy bootstrap + a real tx over the live
  mixnet is a single non-default-feature-gated smoke test the user
  runs by hand, never in CI. **Design consequence (binding): the RNG
  and the transport MUST be injectable seams at the transmit
  component's constructor — no global-RNG calls, no internally
  constructed connector — or the pick/failover logic is untestable.**
- Q15 RESOLVED (2026-07-15): **toggle is a zingolib LightClient API
  with TRI-STATE status (off / bootstrapping / ready)**, driven by
  both zingo-mobile (UniFFI) and zingo-cli (thin `nym on|off|status`
  wrapper). Not a bare bool: "on but not yet reachable" is a real
  state the UI must show, because a send during bootstrapping must
  neither silently wait-then-clearnet nor silently fail. Default on
  for connected sessions; optional `--no-mixnet` startup opt-out.
  ALL 15 DESIGN QUESTIONS NOW RESOLVED — grill complete, drafting the
  capstone ADR.
- Glossary: STARTED — Nym mixnet, Witness Rotation, Broadcast Indexer
  added to zingolib/CONTEXT.md; stale Tor entry corrected. More terms
  graduate on implementation.
- Queued: ADR (capstone, draft after Q15); numbering must dodge the
  OP_RETURN ADR-0010 claim from a sibling worktree.

## Implementation — increment 1 (COMPLETE 2026-07-15): pure seam-B core

VERIFIED: `cargo check -p zingolib --features nym` green (1m04s); the 8
new unit tests pass (`cargo test -p zingolib --features nym --lib nym::`);
the default no-`nym` build is unaffected (`cargo check -p zingolib` green).
Not yet committed — awaiting user go-ahead.

Built: `MixnetMode` tri-state (`nym/mode.rs`); the `Transmitter` trait +
witness-rotation `broadcast` with injected RNG, sequential failover on
`Unreachable`, terminal on `Rejected`, bounded attempts, no-repeat picks
(`nym/broadcast.rs`, 7 tests incl. a determinism/injectability test); the
provisional Broadcast Indexer list separate from the sync list
(`nym/broadcast_indexers.rs`, 1 test). All behind the off-by-default `nym`
feature, no nym crates pulled.


Building the CI-testable core the ADR made binding, behind an off-by-default
`nym` feature, with NO nym crates yet (the live NymProxy transport slots in
behind the `Transmitter` trait in a later increment). File claims for this
increment (mine; others hold only zingo-cli files):

- `zingolib/Cargo.toml` — add `nym = []` feature (minimal, re-read first).
- `zingolib/src/lib.rs` — add `#[cfg(feature = "nym")] pub mod nym;`.
- `zingolib/src/nym/mod.rs` — module root (new).
- `zingolib/src/nym/mode.rs` — `MixnetMode` tri-state (new).
- `zingolib/src/nym/broadcast.rs` — `Transmitter` trait + witness-rotation
  `broadcast` with injectable RNG + failover + unit tests (new).
- `zingolib/src/nym/broadcast_indexers.rs` — provisional curated broadcast
  list, separate from the sync list (new).

## Implementation — increment 2 (IN PROGRESS 2026-07-15): nym-sdk transport

Porting the POC's NymProxy into zingo-netutils behind a `nym` feature and
implementing `Transmitter`. KEY FINDING: the POC pinned a zingolabs FORK of
nym (github.com/zingolabs/nym branch nym_wallet_poc_2_1) — but ADR 0011
forbids fork pins, and upstream now PUBLISHES all three on crates.io
(nym-sdk 1.21.2, nym-http-api-client 1.21.3, nym-validator-client 1.21.3,
none yanked). So we use upstream, satisfying the ratified constraint. File
claims: zingo-netutils/Cargo.toml (+ optional nym deps, `nym` feature),
Cargo.lock (shared — careful), zingo-netutils/src/nym_proxy.rs (new, ported).
GATE FAILED (2026-07-15): nym-sdk 1.21.2 does NOT resolve against the
ironwood stack. Two independent hard conflicts, both reverted cleanly:
- With jwt-simple 0.12.12 (what cargo picks under nym's `^0.12.12`):
  jwt-simple pins `rand =0.8.5`, but nym-bandwidth-controller needs
  `rand ^0.8.6`. Unsatisfiable.
- Steering to jwt-simple 0.12.17 (rand ^0.8.6) moves the conflict to
  **crypto-common**, and this one is FUNDAMENTAL: jwt-simple 0.12.17 →
  superboring 0.1.12 → ml-dsa =0.1.0-rc.11 needs `crypto-common ^0.2`
  (>=0.2.1), but **zcash_primitives 0.29.0 pins `crypto-common
  =0.2.0-rc.1`**. The ironwood zcash crypto foundation and nym's
  post-quantum crypto foundation demand incompatible crypto-common.
The POC only worked because it ran on a pre-ironwood base (different
crypto-common) AND used a zingolabs FORK of nym (github.com/zingolabs/nym)
that patched these pins. Neither holds now, and ADR 0011 forbids the fork
and any [patch]. **BLOCKER — needs a user decision** (see options in the
turn report). Increment 1 is UNAFFECTED: the `Transmitter` trait is
transport-agnostic, so the witness-rotation core stands regardless of how
the mixnet transport is ultimately provided.

"SEPARATE CRATE" DOES NOT HELP (analysed 2026-07-15, corrected): cargo DOES
keep multiple versions of a crate in one Cargo.lock — but only across
semver-INCOMPATIBLE ranges (0.1.x alongside 0.2.x). Within ONE compatibility
range it keeps a single node. Here both crypto-common requirements are in
the SAME 0.2.x range (`=0.2.0-rc.1` from zcash_primitives 0.29 vs `^0.2`
from nym's ml-dsa path), so cargo must unify them and no single 0.2.x
version satisfies both — the resolver failed rather than duplicating. A
separate workspace MEMBER doesn't change this: zingo-netutils already has
ZERO zcash_primitives (verified: `cargo tree -p zingo-netutils -i
zcash_primitives` empty) yet the conflict still fires, because zingo-cli /
libtonode-tests pull both zingolib (zcash) and netutils (nym) into the one
lockfile. Coexistence would require the two crypto-common requirements to be
in DIFFERENT ranges (unachievable — both are pinned deep in 0.2), OR a
separate RESOLUTION unit (own Cargo.lock) linked at RUNTIME across a process
boundary — the out-of-process SOCKS5 daemon (option 1).

## Implementation — increment 2a (COMPLETE 2026-07-15): netutils own lockfile

DONE + VALIDATED. netutils is excluded from the parent workspace, made its
own workspace root (own [workspace.dependencies] + [patch.crates-io] for
lightwallet-protocol git rev and time v0.3.47), and now owns
zingo-netutils/Cargo.lock (160 pkgs). Verified: (1) root workspace resolves
and `cargo check --workspace` is GREEN (15.8s) — main lock shrank 172 lines
(netutils dev-deps no longer in the main lock, expected); (2) netutils
compiles standalone `--all-features` (18.4s); (3) PAYOFF PROVEN — `cargo add
nym-sdk@1.21.2` resolves cleanly in netutils's standalone graph (no
crypto-common conflict, because that graph has zero zcash_primitives). The
validation nym-sdk was then reverted; it lands with the transport code in 2b.
CI (DONE 2026-07-15): added a `netutils-standalone` job to
`.github/workflows/ci-pr.yaml` — fmt --check, clippy --all-targets
--all-features -D warnings, and test --all-features against the netutils
manifest (own workspace, its own cache). Validated locally: YAML valid,
fmt/clippy/test all green (8 unit + 1 doc test). This restores the PR-gate
coverage that `--workspace` (checkmate, cargo-hack, doc tests, nextest
archive) now skips. KNOWN NON-BLOCKING GAPS left as follow-ups: ci-nightly's
`cargo-hack-build` feature-powerset build and coverage.yaml's `llvm-cov
--workspace` still exclude netutils (a nightly build-combo check and
coverage metrics, neither a merge gate).

User directive: "Move zingonetutils the minimum distance to have its own
lockfile. Still listed in the workspace manifest, but no longer as a member."
Rationale: as its own resolution unit, netutils can build STANDALONE with the
`nym` feature (nym-sdk's crypto-common ^0.2 against a graph with ZERO
zcash_primitives — no conflict), while members keep consuming it as a path
dep in the main lock with `nym` off. This is the FOUNDATION for a
standalone/out-of-process nym transport; it does NOT by itself let the main
workspace link nym in-process (a path-dep with nym-on re-merges into the main
lock). Mechanism: root `[workspace]` members drop netutils + `exclude` it;
netutils gets its own `[workspace]`, `[workspace.dependencies]` (mirroring the
root entries it inherits), and `[patch.crates-io]` (BOTH the lightwallet-protocol
git rev AND the time v0.3.47 patch). File claims: Cargo.toml (root, shared —
careful), zingo-netutils/Cargo.toml, zingo-netutils/Cargo.lock (new).
Verify: main workspace still resolves/builds; netutils resolves standalone.

## Implementation — increment 2b (IN PROGRESS 2026-07-16): NymProxy port

Ported the POC's NymProxy into standalone netutils behind the `nym` feature:
netutils/Cargo.toml gains optional nym-sdk/nym-http-api-client/
nym-validator-client/tokio-socks/tokio(time) + the `nym` feature;
error.rs gains NymProxyError (gated); nym_proxy.rs is the ported module
(adapted docs, no GrpcIndexer method refs); lib.rs gates `pub use
NymProxy`. Verified: main workspace still builds with nym OFF (4.9s green).
PORT COMPLETE + GREEN (2026-07-16): standalone `cargo check --features nym`
passes (1m02s) — NO API drift; the POC's fork-era calls
(MixnetClientBuilder::new_ephemeral/socks5_config/connect_to_mixnet_via_socks5,
socks5_url, disconnect, get_all_described_nodes_v2,
node.description.network_requester, nym_http_api_client::Client::builder,
NymApiClientExt) all match upstream nym-sdk 1.21. clippy --all-features
-D warnings clean; test --all-features = 11 passed / 3 ignored (live-Nym) /
0 failed, incl. the 3 new NymProxy unit tests. The CI netutils-standalone
job's --all-features now compiles+lints+tests the nym transport (adds a
nym-sdk compile to that job; cached after first run).

ARCHITECTURE CLARIFIED: NymProxy is a SOCKS5 endpoint provider, NOT the
Transmitter. The Transmitter (submits a tx over a SOCKS5 tunnel via
tokio-socks + tonic) is LIGHT and lives in the MAIN lock — it can be built
in-process. The heavy nym-sdk NymProxy lives standalone. So the two never
share a compile unit; they meet at a runtime SOCKS5 boundary.

## crypto-common conflict — root-cause analysis (2026-07-16)

The clash is a DELIBERATE exact-pin, not accidental version skew — and it is
DURABLE. CORRECTION to an earlier overstatement: zcash_primitives 0.29.0 is a
STABLE release (0.26.0..0.29.0 are all stable, not RCs). What is pre-release is
the RustCrypto crypto FOUNDATION it pins: every stable zcash_primitives from
0.26.0 to 0.29.0 declares `crypto-common = "=0.2.0-rc.1"` (exact) and
`block-buffer = "=0.11.0-rc.3"`, because zcash adopted the new-generation
RustCrypto traits early (digest 0.11.0-pre.9) while that generation is still
pre-release — even though crypto-common itself has since shipped stable 0.2.1/
0.2.2. nym's post-quantum path (jwt-simple → superboring → ml-dsa 0.1.0-rc.11)
uses stable `crypto-common ^0.2` → 0.2.2. Both sit in the one 0.2.x
compatibility bucket, so a single lock picks one node; `=0.2.0-rc.1` and stable
`^0.2` share no solution, and the rc.1→0.2.2 API churn is real (the pre-release
digest 0.11 stack would not compile against 0.2.2). NOT tied to NU6.3/Ironwood
stabilization (those crates are already stable-versioned); the pin dissolves
only when the RustCrypto digest-0.11 line stabilizes and zcash bumps to it —
carried unchanged across FOUR stable zcash_primitives minors, so not imminent.
The two-lockfile split (2a) is therefore a durable architecture, not a
temporary bridge.

### Research synthesis (3 sub-agents, 2026-07-16) — why the pin, can zcash unpin

WHY: librustzcash introduced the exact pins on 2025-02-26 (commit 20e1a705f4ae,
PR #1717, "Avert MSRV breakage in WASM & no-std builds"). Inline comment:
`# later RCs require edition2024`. crypto-common 0.2.0-rc.1 / block-buffer
0.11.0-rc.3 are the LAST edition-2021/low-MSRV pre-releases; the next RC jumps
to edition2024/MSRV-1.85, which then-low-MSRV WASM/no-std builds couldn't take.
That rationale is now STALE — librustzcash is on edition 2024 / MSRV 1.88. The
pin persists by inertia + cohort coherence: `bip32 = "=0.6.0-pre.1"` drags in
the whole -pre RustCrypto cohort (hmac 0.13.0-pre.4 → digest 0.11.0-pre.9 →
crypto-common), and stable bip32 0.6.0 has NEVER shipped (stable tops at 0.5.3,
old trait gen). Notably digest 0.11.0-pre.9's OWN req is caret `^0.2.0-rc.0`
(would accept 0.2.2) — so librustzcash's EXACT `=` pin is the sole thing
forcing rc.1.

CAN UNPIN: the stable RustCrypto stack now EXISTS and coheres (crypto-common
0.2.2, digest 0.11.3, block-buffer 0.12.1; digest 0.11.3 → crypto-common ^0.2).
But it's not a one-liner: crypto-common 0.2.1 removed the BlockSizes trait
(generic-array→hybrid-array), so a `[patch]` to 0.2.2 RESOLVES but FAILS TO
COMPILE against the frozen pre-release digest/block-buffer. A real fix migrates
the whole cohort to stable, gated on upstream shipping stable bip32 0.6.0 /
sha2 0.11 / pbkdf2 0.13 (all still pre-release). No tracking issue, no
maintainer timeline; held across 0.26→0.29. Ecosystem precedent: Zebra (PR
#10522) TOLERATES the RC crates via deny.toml skip-tree, does not unpin. And
ml-dsa already moved to crypto-common ^0.3 — zcash is two generations behind,
so even a future move to 0.2.x may not close the gap with newer nym.

VERDICT: do NOT architect around an imminent unpin; a `[patch]` is not viable
(compile failure). The split-lockfile is correct and DURABLE — it survives
zcash landing on 0.2.x OR 0.3.

## Implementation — increment 2d (IN PROGRESS 2026-07-16): SOCKS5 transmit

netutils gains a light `socks5-transmit` feature (tokio-socks + hyper-util +
tower, NO nym-sdk) and `send_transaction_via_socks5(socks5_addr, indexer,
raw_tx, height, timeout)` — dials the indexer through a local SOCKS5 proxy via
tonic connect_with_connector, returns the txid, classifies errors as
Unreachable (failover) vs Rejected. zingolib's `nym` feature now enables
`zingo-netutils/socks5-transmit`. GATE PASSED: `cargo check -p zingolib
--features nym` GREEN in the MAIN lock (15.8s; +16 lines Cargo.lock) — the
wallet-side transport resolves in-process, no crypto-common conflict.
netutils standalone `--features socks5-transmit` also green. NEXT: a
SocksTransmitter in zingolib implementing the increment-1 Transmitter trait
(wraps send_transaction_via_socks5, maps to SubmitError); the proxy supervisor
that spawns nym-proxy and parses SOCKS5_ADDR=; wire the broadcaster into
transmit_transactions; the tri-state toggle LightClient API.

## Consumption model — RATIFIED (A) bundle-and-spawn (2026-07-16)

User chose A: the wallet ships a nym-proxy binary (built from standalone
netutils) and SPAWNS it as a child process, then dials its local SOCKS5
port. The wallet owns the proxy lifecycle (start, health, shutdown). This
means:
- netutils-standalone gains a `[[bin]]` (e.g. `nym-proxy`) that runs a
  NymProxy and prints/serves its SOCKS5 address, built in netutils's own
  lockfile with the nym stack.
  DONE (increment 2c, 2026-07-16): `zingo-netutils/src/bin/nym-proxy.rs`
  — starts a NymProxy, prints `SOCKS5_ADDR=127.0.0.1:PORT` on stdout, serves
  until ctrl_c, then disconnects. `[[bin]]` with `required-features = ["nym"]`;
  nym tokio features expanded (rt-multi-thread, macros, signal). Verified:
  builds standalone `--features nym --bin nym-proxy` (green), clippy
  --all-targets --all-features -D warnings clean, main workspace unaffected.
  ADR 0011 amended (2026-07-16) with the out-of-process/spawned-child model.
- The wallet side (main lock) spawns that binary, reads its SOCKS5 address,
  and the SOCKS5-dialing Transmitter (tokio-socks + tonic, main lock) routes
  send_transaction through it. Tri-state Mixnet Mode maps to the child's
  lifecycle: Off = not spawned, Bootstrapping = spawned+connecting, Ready =
  SOCKS5 reachable.
- BUILD/PACKAGING: the wallet build must produce and bundle the nym-proxy
  binary from the separate workspace. How the binary is located at runtime
  (bundled path vs PATH vs built-on-demand) is an implementation detail to
  settle when wiring.
- ADR 0011's "embedded mixnet SOCKS5" line to be amended: the mixnet proxy
  is a spawned child process, not linked in-process (dep reality: nym-sdk's
  crypto-common ^0.2 cannot share the main lock).

## Implementation — increment 3 (STEP 1 DONE 2026-07-16): unify transmit policy

DONE + VERIFIED (commit 0da43c3c9): `transmit::resilient_transmit` is the sole
definition of the retry/duplicate-in-mempool/queued-probe policy — generic
over `TransmitTarget`, pure (no wallet mutation), injectable sleep. 8
regression tests green (accept, mempool/in-chain duplicate, queued
settle/exhaust, retry succeed, delivery-check confirm/deny), clippy -D
warnings clean, workspace check green. `transmit_transactions` rewired to
`ClearnetTarget` + `resilient_transmit`, clearnet behavior unchanged.
STEP 2a DONE (commit e236acd17): SOCKS5 delivery-check
(transaction_known_via_socks5) + shared connect_via_socks5 helper.

STEP 2b DEFERRED (2026-07-16, user direction "defer this work until later"):
the broadcaster failover policy. Key insight to preserve — the client CANNOT
assume zainod vs lightwalletd (or other) on the wire, so failover must be a
CLIENT-SIDE policy driven by attempt counts, NOT by classifying server error
strings (this also argues against introducing a message-substring "Rejected"
classification at all). User floated an ESCALATING FAN-OUT: on a failure,
attempt 2 more indexers, then 3, then 4 (1+2+3+4...) across the ~10 broadcast
indexers. TENSION TO RECONCILE when resumed: this fires the same tx to
multiple indexers in PARALLEL, which contradicts the earlier ratified "single
random pick per send, never redundant" decision (Q10). Both are defensible;
resolve the redundancy/privacy vs robustness trade-off first. Then: SocksTarget
(submit=send_transaction_via_socks5, knows=transaction_known_via_socks5),
retire the increment-1 simplistic `Transmitter`/`SubmitError`, branch
transmit_transactions on Mixnet Mode (needs the toggle + supervisor), delete
GrpcIndexer's `// TODO; add nym_client`.

## Implementation — increment 3 design notes

De-duplicate the retry / duplicate-in-mempool / queued-probe orchestration
so it is UNIQUELY defined, regression-tested, and reused by both the clearnet
indexer path and (later) the Nym broadcaster. Design:
- `zingolib/src/lightclient/transmit.rs`: `TransmitTarget` trait (submit +
  knows_transaction) + `resilient_transmit()` — the sole definition of the
  policy, pure (no wallet-state mutation), with an injectable sleep hook so
  the probe/retry cadence is unit-testable without real time. Constants
  MAX_RETRIES / MAX_QUEUED_PROBES move here. Full regression tests (accept,
  duplicate-in-mempool, in-chain, queued-probe settle/exhaust, retry
  succeed/exhaust, delivery-check confirm/deny).
- `send.rs`: `ClearnetTarget` wraps GrpcIndexer (send_transaction +
  get_transaction); `transmit_transactions` rewired to call
  `resilient_transmit` and do wallet-state effects around it —
  behavior-preserving.
- Step 2 (next): rework the increment-1 witness-rotation broadcaster to build
  a SocksTarget per pick and run the SAME resilient_transmit, retiring the
  increment-1 simplistic `Transmitter` trait; branch transmit_transactions on
  Mixnet Mode; delete GrpcIndexer's stale `// TODO; add nym_client`.

## Implementation — increment 4 (DONE 2026-07-16): Mixnet Mode toggle + supervisor

Proxy supervisor: `zingolib/src/nym/supervisor.rs` — `MixnetProxy::spawn`
spawns the bundled nym-proxy child (kill_on_drop), a background task reads its
stdout and drives the tri-state (Bootstrapping → Ready on SOCKS5_ADDR=, → Off
if stdout closes without one = fail-closed). `mode()`, `socks5_addr()`,
`stop()`. The stdout state machine (`drive_state`) is generic over the reader,
unit-tested with byte-slice scripts (6 tests: parse/trim/ignore, ready-on-
announce, ready-after-preamble, off-on-close). The `SOCKS5_ADDR=` prefix is now
one shared `zingo_netutils::SOCKS5_ADDR_LINE_PREFIX` const (binary emits,
supervisor parses).

Toggle: `LightClient::{enable_mixnet(binary_path), disable_mixnet,
mixnet_mode, mixnet_socks5_addr}` behind the `nym` feature — a `#[cfg]` field
`mixnet_proxy: Option<MixnetProxy>` (None = Off). enable spawns/replaces,
disable stops (consent-gated clearnet), mixnet_mode maps None→Off else the
proxy tri-state. Verified: default build unaffected (field/methods gated out),
nym build + clippy -D warnings green.

CONSUMER POLICY (not here): the forced-on-at-startup-for-connected-sessions /
skip-under-`--offline` / never-persist policy is the consumer's to apply
(zingo-cli calls enable_mixnet at startup with the bundled binary path). The
binary's runtime location (bundled path) is packaging, TBD.

Later increments: branch transmit_transactions on mixnet_mode (+ the deferred
broadcaster failover policy); the reqwest+SOCKS5 price path; CLI
`nym on|off|status`.

## File claims (prospective, gated on ratification)

- `.agent-plans/nym-transmission.md` (this file)
- `zingo-netutils/` — `nym` feature, ported `nym_proxy.rs`, connector
  wiring.
- `zingolib/src/lightclient*` — routing wiring.
- `zingo-price/` — proxy-aware fetch.
- `zingo-cli/` — flag surface, server_select behavior, stale `--tor`
  help-text fix.
- `zingolib/CONTEXT.md` — glossary entries as terms resolve.
- `docs/adr/` — new ADR on the Nym transport decision.

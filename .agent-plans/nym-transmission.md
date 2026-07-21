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

STEP 2b RESOLVED (2026-07-17): the broadcaster failover policy is the
ESCALATING, SERIALLY GATED FAN-OUT, capped at SIX distinct indexers. User chose
the escalating fan-out over sequential single-pick (my recommendation),
overriding — deliberately — the earlier thrice-stated "single random pick, never
redundant" absolute (Q10). PURPOSE (user, 2026-07-17): robustness to CENSORSHIP
— a Broadcast Indexer may accept the connection but suppress the relay or
misreport, and the fan-out routes the same send around it to honest indexers.

Resolved shape (user-clarified 2026-07-17, "1 in-flight, then 2, then 3 in
SERIAL"):
- Rounds are serial GATES; parallelism is only WITHIN a round. Round r launches
  r fresh random indexers in parallel, and round r+1 launches ONLY after ALL r
  arms of round r fail to confirm delivery. So: round 1 = 1 arm; round 2 = 2
  parallel (only after round 1 fails); round 3 = 3 parallel (only after BOTH of
  round 2 fail).
- Cap = 6 distinct indexers. The 1+2+3 schedule reaches 6 at the end of round 3,
  so a fourth round (=4) never runs. Cumulative: 1, 3, 6.
- Witness rotation is PRESERVED on the happy path: round 1 is a single random
  pick, so a first-try success contacts exactly one witness. Redundancy is
  accepted only on the failure path.
- Within a round, the FIRST arm to confirm delivery wins; the rest are
  abandoned.
- A round "fails" (escalates) on ANY outcome short of confirmed delivery — a
  transport failure, a refusal, or silence — because a censoring indexer can
  present any of these. NO error-string "Rejected" classification; retire
  broadcast.rs's `SubmitError::Rejected`.
- Success = delivery: accepted submission, duplicate-in-mempool/chain, or a
  delivery check that finds the txid known. Because a censoring indexer can
  misreport delivery, prefer an INDEPENDENT confirmation (a different indexer,
  or the client's own sync view of the public mempool) where available —
  implementation refinement to pursue, not over-specified here.
- The per-submission retry / duplicate-in-mempool / queued-for-download handling
  is the SHARED `resilient_transmit` policy (increment 3), reused UNCHANGED per
  arm; the fan-out orchestrator drives the shared per-indexer policy across
  rounds rather than duplicating it. Each arm runs
  `resilient_transmit(SocksTarget(indexer), ...)`.

Implementation shape (next, on user go-ahead):
- `SocksTarget` in zingolib implementing `TransmitTarget`
  (submit=send_transaction_via_socks5, knows=transaction_known_via_socks5) over
  the supervisor's SOCKS5 address.
- A fan-out orchestrator in the nym broadcast module: shuffle the curated list
  once (repetition-free random order), drive the serially-gated rounds (1,2,3;
  cap 6), race each round's arms (first delivery wins, abandon the rest),
  escalate only on whole-round failure, surface failure at the cap.
- RETIRE broadcast.rs's `Transmitter`/`SubmitError`/`broadcast` (the sequential
  single-pick logic) — folded into the orchestrator over `resilient_transmit`.
- Branch `transmit_transactions` on Mixnet Mode via `mixnet_route()` (increment
  5): Clearnet -> ClearnetTarget + resilient_transmit (today's path); Mixnet ->
  the SocksTarget fan-out; Bootstrapping -> fail closed.
- Tuning note: per-arm same-target retries (resilient_transmit's MAX_RETRIES=3)
  may want to be lower for the mixnet, since the fan-out's escalation IS the
  "try elsewhere" mechanism and hammering one dead mixnet indexer three times
  only delays escalation. Consider a per-arm attempt bound / shorter per-attempt
  timeout. Not a policy decision; settle when wiring.
- Delete GrpcIndexer's stale `// TODO; add nym_client`.

## Implementation — increment 6 (DONE 2026-07-17): the fan-out, wired into send

The resolved escalating fan-out is built and branched into the send path.

`nym/broadcast.rs` REWRITTEN: the increment-1 `Transmitter`/`SubmitError`/
`broadcast` (sequential single-pick, with the retired `Rejected` classification)
is gone, replaced by `fanout_broadcast(indexers, rng, cap, run_arm) ->
Result<String, FanoutError>`. It shuffles once (repetition-free random order),
drives serially-gated rounds (round r = r parallel arms, entered only after all
of round r-1 fail), races each round with `futures::future::select_ok` (first
delivery wins, abandon the rest; whole-round failure escalates), and stops at
`MAX_BROADCAST_WITNESSES = 6` (1+2+3). `run_arm` and the RNG are injected; 6 unit
tests (empty, round-1-single-witness, escalation contacts 1+2=3, all-fail caps at
6 distinct, cap bounded by list length, seed reproduces the round-1 witness). No
error-string classification survives — success is whatever the arm returns Ok.

`send.rs` WIRED: `SocksTarget` (nym-gated) implements `TransmitTarget` over
`send_transaction_via_socks5` / `transaction_known_via_socks5`, so each fan-out
arm runs the SAME `resilient_transmit` policy as clearnet — the shared per-
submission logic is reused, not duplicated. `transmit_one_transaction(Option<&str>
socks5_proxy, ...)` branches: None -> ClearnetTarget + resilient_transmit (today's
path, unchanged); Some(addr) -> `mixnet_fanout_transmit` (nym). `transmit_transactions`
resolves the route ONCE via `mixnet_route()?` before taking the wallet lock, so
Bootstrapping fails closed before any submission; Clearnet -> None, Mixnet(addr)
-> Some(addr). Wallet-state effects stay in the loop around the pure transmission.
Without `nym` the route is always clearnet and the mixnet arms are cfg'd out, so
the default build and the clearnet send path are byte-for-byte behavior-preserving.

Retired `GrpcIndexer`'s stale `// TODO; add nym_client` (the transport is
out-of-process SOCKS5, not an in-struct client; sync stays clearnet-only).

VERIFIED: default `cargo check -p zingolib` green; nym `cargo clippy -p zingolib
--features nym --all-targets -D warnings` green; 18 nym lib tests pass (incl. the
6 fan-out tests); offline send tests (send::built_transaction_shape) 5/5 green on
the default build, confirming the clearnet path is unchanged; fmt clean on both
the root workspace and the netutils workspace.

Still open (not blocking): the CLI `nym on|off|status` surface + the consumer
forced-on-at-startup wiring (zingo-cli calls enable_mixnet with the bundled
binary path); the nym-proxy binary's runtime/bundled location (packaging); the
per-arm same-target retry tuning for the mixnet (resilient_transmit's
MAX_RETRIES=3 may be high when the fan-out is the "try elsewhere" mechanism); and
the independent delivery confirmation refinement (a censoring indexer can
misreport, so cross-check against a different indexer or the sync mempool view).

## Implementation — increment 7 (DONE 2026-07-17): the CLI nym command + nym CI

`zingo-cli` gains a `nym = ["zingolib/nym"]` feature (off by default) and a
`nym` wallet command wrapping the increment-4 LightClient toggle:
- `nym status` (or bare `nym`) -> maps `mixnet_mode()` to a readable line: off,
  bootstrapping, or ready with the SOCKS5 address.
- `nym on [binary_path]` -> `enable_mixnet(path)`; the path is the explicit arg,
  else `$ZINGO_NYM_PROXY`, else `nym-proxy` (PATH). Reports bootstrapping, or the
  spawn error.
- `nym off` -> `disable_mixnet()` (the deliberate per-session clearnet consent).
The command is registered UNCONDITIONALLY in get_wallet_commands; its body is
cfg-split, so a no-`nym` build still lists it and returns "rebuild with
--features nym" instead of "unknown command". Rode along: fixed the stale `--tor`
line in `current_price` help (price now goes over the mixnet, not Tor).

CI: added a `nym-feature` job to ci-pr.yaml — `cargo clippy -p zingolib -p
zingo-cli --features nym --all-targets -D warnings` plus `cargo test -p zingolib
--features nym --lib nym::`. This is the FIRST CI coverage of the main-workspace
nym build; it protects increments 1-7 (the whole nym-gated surface), which the
default --workspace jobs never compiled. No draft gate, so it runs on PR #2470.

VERIFIED: default `cargo check -p zingo-cli` green (command lists, reports
feature-absent); `cargo clippy -p zingo-cli --features nym --all-targets -D
warnings` green; fmt clean; ci-pr.yaml parses.

## Implementation — increment 8 (DONE 2026-07-17): forced-on-at-startup policy

zingo-cli now forces Mixnet Mode on at startup for connected sessions (ADR 0011,
Q8/Q13). Two new startup flags (defined unconditionally so the arg surface is
stable across builds): `--no-mixnet` (the ratified opt-out) and `--nym-proxy
<path>` (explicit binary path). Threaded through `ConfigTemplate`
(`no_mixnet`/`nym_proxy_path`, cfg_attr-allowed dead when nym is off).

In `startup`, gated on the `nym` feature: when `communication_mode == Online`
and `!no_mixnet`, it resolves the proxy path (shared `commands::resolve_proxy_path`:
--nym-proxy > $ZINGO_NYM_PROXY > `nym-proxy` on PATH) and calls
`enable_mixnet` right after LightClient creation, BEFORE sync, so the bootstrap
overlaps sync. The off-state is never persisted (this runs every launch);
`--offline` sessions never transmit and skip it (communication_mode == Offline).

FAIL-CLOSED at startup: a spawn failure ABORTS the session (returns an
io::Error) with an actionable message (install the binary / --nym-proxy / set
$ZINGO_NYM_PROXY / --no-mixnet), rather than quietly proceeding to send over
clearnet. This is the fund-safety reading of the fail-closed invariant. A
successful spawn logs "bootstrapping"; the session proceeds while the proxy
connects.

KNOWN UX caveat (not a safety issue, noted for a later refinement): a one-shot
command that transmits (e.g. `zingo-cli send ...`) issued immediately at startup
can hit the bootstrap window and fail closed with MixnetNotReady; the user
retries once `nym status` shows ready. Interactive sessions overlap the bootstrap
with sync, so send is typically ready by the time it's invoked. A future
`--waitmixnet` (await-ready-before-one-shot) could close this.

VERIFIED: default `cargo check -p zingo-cli` green; `cargo clippy -p zingo-cli
--features nym --all-targets -D warnings` green; 87 cli lib tests pass (arg
parsing + ConfigTemplate::fill); fmt clean.

Still open (not blocking): the nym-proxy binary's bundled runtime location
(packaging — how the wallet build produces and locates the binary); the per-arm
retry tuning and the independent-delivery-confirmation refinement noted above;
an optional `--waitmixnet` for one-shot sends.

## Implementation — increment 9 (DONE 2026-07-17): nym-proxy packaging

The nym-proxy binary is built in the SEPARATE zingo-netutils workspace (own
lockfile, nym-sdk stack), so `cargo build -p zingo-cli` never produces it. This
increment closes the produce-and-locate gap two ways:

RUNTIME (zero-config discovery): `resolve_proxy_path` (zingo-cli/commands.rs)
gains a step — after `--nym-proxy` and `$ZINGO_NYM_PROXY`, before the bare-PATH
fallback, it looks for a `nym-proxy` sitting BESIDE the running executable
(`current_exe().parent()/nym-proxy` + EXE_SUFFIX). So a packaged wallet with the
proxy dropped next to it needs no flag or env var. Help text (the `nym on`
command and the `--nym-proxy` arg) updated to the new precedence.

BUILD TOOLING (produce + place): new workbench binary
`tools/workbench/src/bin/bundle-nym-proxy.rs` (Rust, std-only, per the
tooling-in-workbench rule) — runs `cargo build --manifest-path
zingo-netutils/Cargo.toml --features nym --bin nym-proxy [--release]`, then
copies `zingo-netutils/target/<profile>/nym-proxy` to the main
`target/<profile>/` (or `--dest <dir>`), where the runtime discovery finds it.
`parse_dest` unit-tested (3 tests). cargo-make task `[tasks.bundle-nym-proxy]`
is the thin glue: `makers bundle-nym-proxy [--release] [--dest <dir>]`.

VERIFIED: workbench binary builds + 3 unit tests pass; workbench clippy -D
warnings clean; default `cargo check -p zingo-cli` green; `cargo clippy -p
zingo-cli --features nym --all-targets -D warnings` green; Makefile.toml parses;
fmt clean (zingo-cli + workbench). The actual end-to-end bundle run (building the
nym-sdk stack) is user-driven via `makers bundle-nym-proxy`; the underlying
netutils `--features nym --bin nym-proxy` build was proven green in increment 2c.

The Nym mixnet arc (increments 1-9) is now end-to-end: build zingo-cli with
`--features nym`, run `makers bundle-nym-proxy` to place the proxy beside it, and
a connected session transmits + fetches price over the mixnet by default (forced
on at startup, fail-closed), with `nym on|off|status` for runtime control.

Still open (genuine follow-ups, none blocking): the per-arm mixnet retry tuning;
the independent-delivery-confirmation refinement against a misreporting indexer;
an optional `--waitmixnet` for one-shot sends; and release-packaging integration
(having the release/distribution build invoke bundle-nym-proxy so shipped
artifacts carry the proxy).

## Increment 12 (DONE 2026-07-21): populate the Broadcast Indexer list

RESULT: three-way discovery sweep (hosh 2026-04-18 archive snapshot — the
live tracker is down with a Cloudflare 521; wallet-source server lists
across 12 repos; forum/ZecHub web sweep) produced 130 candidate endpoints;
a live GetLightdInfo probe (grpcurl, TLS + plaintext fallback) found
exactly 19 alive on mainnet — all lightwalletd, all at the same tip
(3420363), every zaino deployment dead, the lightwalletd.com and
zcash-infra.com fleets dead, and 8 of the 10 provisional placeholder
entries dead. Deduped per Q2 to 14 operators (zec.rocks and stardust.rest
each collapse to one entry; the 14 resolve to 14 unrelated IPs — sybil
caveat recorded in the module docs for the vetting issue). New regression
test pins one-endpoint-per-operator by registrable domain. Witness
Rotation + Broadcast Indexer glossary entries updated per Q3 (fan-out
wording, operator-diversity sentence); the CONTEXT.md hunk is left
UNCOMMITTED because the sealed-wallet session holds uncommitted edits
elsewhere in that file. Mainnet fact from the probe: NU6.3 activation
height 3428143, tip 3420352 at probe time — Ironwood not yet active.
Verified: fmt clean, clippy --features nym --all-targets -D warnings
clean, 22 nym lib tests green.

Grilled and ratified (3 questions, 2026-07-21): (Q1=A) the full discovered
set of currently-live mainnet indexer endpoints REPLACES the provisional
`BROADCAST_INDEXERS` and becomes the witness-rotation pool — dead
discoveries are recorded in comments, not the pool; (Q2=A) ONE endpoint
per OPERATOR (rotation's accumulating party is the operator, not the DNS
name), with an operator's other regional variants documented in the module
comment block for the vetting issue; (Q3=A) the stale Witness Rotation
glossary entry (still claims "never fans out", contradicting the ratified
1-2-3 escalating fan-out) is fixed surgically in this session.

File claims for this increment (mine):
- `zingolib/src/nym/broadcast_indexers.rs` — the populated list + provenance.
- `zingolib/CONTEXT.md` — ONLY the Witness Rotation / Broadcast Indexer
  entries (the sealed-wallet session holds uncommitted edits in OTHER
  sections of this file; re-read before edit, stage explicit paths).
- `.agent-plans/nym-transmission.md` (this file).

## Merge-readiness phase (grill, 2026-07-21)

RATIFIED: two merge gates for PR #2470 — (1) the three-stage live smoke
ladder (netutils ignored tests -> bundled binary -> full wallet session),
(2) the PR title/description refresh + review request. Three follow-up
issues to file: broadcast-list operational vetting, fan-out hardening
(per-arm retry tuning + independent delivery confirmation), distribution
UX (--waitmixnet + release packaging).

PROGRESS (2026-07-21, after increment 12): the follow-up issues are FILED
— #2498 (broadcast-list vetting, seeded with the sweep results + sybil
caveat), #2499 (fan-out hardening), #2500 (distribution UX). PR #2470's
title/description REFRESHED to describe the full arc ("feat: transmit and
fetch price over the Nym mixnet (ADR 0011)"). Branch state: local
reboot_nym (98f644eed) strictly supersedes zingolabs/reboot_nym (verified:
no remote commit lacks a local patch-equivalent; the remote tip f1147a737
is the pre-repair line with red CI), so the PR update is a user-run
`git push --force-with-lease zingolabs reboot_nym`. Remaining gates, all
user-driven: the push; smoke stage 1 RE-RUN over the live mixnet (the
2/3 pass predates the increment 10-11 planner rewrite); stage 2 (makers
bundle-nym-proxy, run the bundled binary); stage 3 (full wallet session
over the mixnet); then un-draft + review request.

SMOKE STAGE 1 RE-RUN: PASSED (user-run 2026-07-21, post-planner): 3/3 —
disconnect_clean 5.4s, starts_and_reports_address 5.8s (the previous
120s-starvation failure, now bootstrapping in one hedge window),
socks5_tunnel_works 6.8s. The hedged racing planner is validated against
the live mixnet. Remaining: the push, stages 2-3, un-draft + review.

SMOKE STAGE 1 RESULT (user-run, cargo nextest -- --ignored): 2/3 passed —
the live SOCKS5 tunnel through the mixnet to zec.rocks:443 works (~8s
bootstraps). nym_proxy_starts_and_reports_address FAILED at the 120s
lifecycle cap: connect_with_retries had NO per-attempt timeout, so one
unresponsive provider (time-entropy shuffle draw) starved the whole
budget and the retry engine never reached provider #2. Production impact:
the forced-on-at-startup path would abort fail-closed after 120s on a
bad draw.

## Implementation — increment 10 (DONE 2026-07-21): per-attempt timeouts

nym_proxy.rs: PER_ATTEMPT_CONNECT_TIMEOUT = 20s wraps each
start_with_config call inside the connect_across_providers closure (the
pure engine is untouched); DISCOVERY_TIMEOUT = 15s bounds
get_all_described_nodes_v2. Each attempt now binds a FRESH port (a
timed-out attempt may still hold its port); reconnect_inner inherits
that, taking bind_port from the connected proxy. New
NymProxyError::AttemptTimeout variant. New deterministic regression test
timed_out_attempt_advances_to_next_provider (tokio start_paused; dev-only
tokio test-util feature added) pins: a hanging provider costs exactly one
per-attempt timeout, then the engine advances. Verified: netutils clippy
--all-targets --all-features -D warnings clean; --features nym lib tests
24 pass / 3 ignored; default-build tests pass; fmt clean.

RODE ALONG — rebase fallout repaired (pre-existing, CI red at f1147a737):
the reboot_nym copy of the "repair the rebase fallout" commit (22daf298f)
lost the lightclient.rs hunk that its sibling (view-model's c66553fdd)
carried, so LightClient was missing the `#[cfg(feature = "nym")]
mixnet_proxy` field while all its uses survived — `cargo check -p
zingolib --features nym` was BROKEN at HEAD and the nym-feature CI job
red. Restored the field + the three constructor initializations (declaration
recovered verbatim from c66553fdd). Verified: zingolib+zingo-cli nym
clippy -D warnings green, default check green, 18 nym lib tests pass,
fmt clean.

RESOLVED (user, 2026-07-21): "Build it now" — plus two directives: (A)
capture and leverage ALL per-attempt information, (B) capture the
commonalities with the send fan-out under proper DRY logic. Also:
identify testable hypotheses and write falsifying tests as work proceeds.

## Implementation — increment 11 (DONE 2026-07-21): shared racing planner

THE DRY SHAPE (B): one pure planner, two thin effectful drivers.
`zingo-netutils/src/arm_race.rs` (NEW, ungated, pub — netutils is a path
dep of zingolib, so both crates share it) is a pure state machine:
`RaceState::{start, on_event} -> Vec<RaceAction>` over
`RaceEvent::{ArmFailed, HedgeElapsed}` with the escalation style as data —
`LaunchPolicy::Hedged{max_parallel, hedge_interval}` (bootstrap) vs
`LaunchPolicy::EscalatingRounds` (the ratified 1-2-3 serially gated send
fan-out). The planner owns candidate allocation (no repeats), the cap,
in-flight accounting, failure accumulation, GiveUp detection, progress
snapshots (`RaceProgress`, Display), and `failure_summary`. The drivers
stay separate BY DESIGN: netutils' `drive_race` (nym_proxy.rs, nym-gated)
runs tokio JoinSet tasks ('static arms natural there; aborts losers,
disconnects a simultaneous second winner); zingolib's `fanout_broadcast`
drives FuturesUnordered over BORROWED arm futures (spawning would force
Arc/'static onto wallet state, and unifying drivers would need a futures
dep in netutils — barred). connect_with_retries + its 5 tests DELETED
(superseded); mixnet_connect.rs keeps strip_socks5_scheme/seeded_shuffle.

INFORMATION LEVERAGE (A): every arm outcome is retained, not just the
last error (the old engine's loss). In-race leverage: a failure launches
a replacement IMMEDIATELY (fail-fast beats the hedge timer); failed
candidates are never retried; the terminal error names every attempted
provider and its failure (NymProxyError::AttemptsExhausted; FanoutError::
AllFailed.last_message -> summary naming every witness). Live progress:
NymProxy::start_with_progress(FnMut(String)) -> the binary prints
NYM_STATUS= lines (shared NYM_STATUS_LINE_PREFIX const beside the
SOCKS5_ADDR one) -> supervisor drive_state captures the latest into
bootstrap_detail (cleared on Ready) -> LightClient::
mixnet_bootstrap_detail() -> `nym status` renders "bootstrapping —
attempt 4/10: 2 in flight, 2 failed". DEFERRED leverage (noted, not
built): latency-adaptive hedge interval and cross-session provider
scoring need persistence + live data the vetting issue will produce.

Bootstrap policy: MAX_PARALLEL_CONNECTS=3, HEDGE_INTERVAL=5s;
PER_ATTEMPT_CONNECT_TIMEOUT=20s and DISCOVERY_TIMEOUT=15s (increment 10)
stay per arm; NYM_LIFECYCLE_TIMEOUT=120s stays the outer cap. Worst-case
all-dud walk of 10 providers ~65s virtual, inside the cap. MAX_CONNECTION
_ATTEMPTS + SYSTEM_SLEEP_MILLIS retired with the round-loop.

HYPOTHESES -> FALSIFYING TESTS (all green): planner (10 ungated tests in
arm_race.rs: replacement-on-failure, hedge widens to max_parallel then
stops, round gate holds mid-round, 1-2-3 escalation, caps, GiveUp,
full-account summary, progress render); driver (5 paused-time nym-gated
tests: hedge rescues a hanging provider at ~5s not 20s, per-attempt
timeout frees a wedged slot at max_parallel=1, lost race accounts for
every attempt, simultaneous second winner handed to abandon/disconnect
not leaked, progress lines narrate failures) + short_provider_name
truncation; fan-out (the 6 ratified falsifiers UNCHANGED and green over
the planner rewrite + new every-witness-named summary test); supervisor
(2 new: status line updates detail while Bootstrapping, address line
still wins and clears detail; 6 old green); CLI (render_status pinned
byte-identically incl. new detail case + stale-detail-never-on-ready).

VERIFIED: netutils standalone fmt/clippy --all-targets --all-features -D
warnings/test default + --features nym (34 pass / 3 ignored live);
main workspace clippy -p zingolib -p zingo-cli --features nym -D warnings
clean; zingolib nym:: 21 pass; zingo-cli lib 98 pass; cargo check
--workspace green; fmt clean. zingolib/CONTEXT.md deliberately untouched
(the sibling sealed-wallet session holds uncommitted edits there).

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

## Implementation — increment 5 (DONE 2026-07-17): price-fetch over mixnet

The price surface (Decision 2's other required-mixnet surface) now routes
through the mixnet, chosen over the send branch because price has no failover
policy — a single Gemini GET, so the DEFERRED broadcaster fan-out debate does
not block it.

New SHARED abstraction: `nym/route.rs` — `resolve_route(mode, socks5_addr) ->
Result<MixnetRoute, MixnetNotReady>` names the fail-closed policy ONCE. Ready →
`Mixnet(addr)`; Off → `Clearnet` (deliberate toggle-off consent);
Bootstrapping (or Ready-without-address) → `Err(MixnetNotReady)` — refuse, never
leak. `MixnetRoute::socks5_proxy()` shapes it for a proxy-aware client. Both
send (later) and price consume this one resolver; 5 unit tests. Exposed on
LightClient as `mixnet_route()` (nym-gated).

Mechanism (pure, no policy): `zingo_price::get_current_price(Option<&str>)`
builds the reqwest client with `.proxy(socks5h://addr)` when Some — `socks5h`
resolves the hostname AT the proxy so DNS never leaks to the clearnet resolver;
reqwest gains the `socks` feature. Threaded through
`PriceList::update_current_price(Option<&str>)` and
`Wallet::update_current_price(Option<&str>)`.

Policy (the caller): `LightClient::update_current_price()` (NON-gated) resolves
`mixnet_route()?` under `nym`, else clearnet, and delegates. New
`LightClientError::{PriceError, MixnetNotReady (nym-gated)}`. The CLI
`updatecurrentprice` now calls the LightClient method instead of reaching into
the wallet, so it inherits the mixnet policy. Verified: default build green
(price always clearnet), nym clippy -D warnings green, 5 route tests pass, fmt
clean.

Remaining: branch transmit_transactions on mixnet_mode (blocked on the DEFERRED
broadcaster failover policy); CLI `nym on|off|status` + the consumer
forced-on-at-startup wiring; the nym-proxy binary's runtime/bundled location.

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

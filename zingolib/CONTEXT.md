# zingolib Domain Glossary

> This file is a pure glossary. No implementation details, no specs, no plans.

---

## Chain

**ChainType** — The network a wallet connects to: `Mainnet`, `Testnet`, or `Regtest`. `Regtest` carries `ActivationHeights` to configure protocol upgrade heights locally.

**Block Height** — A `u32`-compatible integer identifying a block's position on the chain. Used as the primary ordering key for sync progress.

**Birthday** — The earliest block height at which a wallet may have received funds. Scanning begins here on first sync. Birthday is a property of a wallet, never of a pool or an upgrade: a pool begins at its Pool Activation, and a network upgrade has an activation height. *Avoid*: pool birthday.

**Library Birthday** — A per-ChainType block height baked into each zingolib release, chosen so that it had already been mined when the release was cut. Any wallet a given release creates necessarily post-dates the release, so this height is a safe Birthday for a NewSeed wallet created while Indexerless, where no Indexer can report the chain height. It never applies to restores: a restored seed or viewing key may predate the library, so restoring always requires a caller-supplied Birthday. The caller passes it explicitly; the library only publishes the value.

**Chain Tip** — The range of blocks at the top of the blockchain starting from the lowest block containing the last note commitment to the most recent shard, up to the current chain height. Distinct from "chain height" (highest known block).

**Spend-Evidence Height** — The block height through which the wallet's evidence of spends and transaction inclusion is complete: every block at or below it has been scanned with its nullifiers mapped, so a note spent or a transaction mined at or below this height is provably visible. Distinct from chain height (highest known header, which may exceed what scanning has reached) and from the highest scanned block (scan ranges complete out of order, and a scanned-but-unmapped block still lacks spend evidence). Judgments that condemn — declaring a transaction expired-unmined, a migration part dead — bind to the Spend-Evidence Height; judgments that plan forward may use chain height.

**Re-org** — A chain reorganisation. The sync engine detects this by checking block hash continuity in a verification window at the top of the locally known chain.

---

## Pools and Outputs

**Pool** — A shielding protocol or address type. Four pools exist: `Transparent`, `Sapling`, `Orchard`, and `Ironwood`. The Ironwood pool activates at NU6.3 and succeeds Orchard; the Orchard→Ironwood migration (ZIP 318) moves value between them.

**Pool Activation** — The block height at which a shielded pool begins to exist on the chain, derived solely from the chain's consensus parameters through a single pool-to-network-upgrade mapping: Sapling activates at the Sapling upgrade, Orchard at NU5, Ironwood at NU6.3. Every height clamp involving a pool's existence — the witness guard's clamp of the wallet Birthday to the Pool Activation, the migration schedule's activation floor — is a derivation from Pool Activation, never an independent mapping. A pool is never said to have a Birthday (see Birthday).

**Ironwood** — The shielded pool introduced by NU6.3. An Ironwood note is addressed through the same receiver as an Orchard note: the Ironwood receiver of a Unified Address *is* its Orchard receiver. Under the ZIP 318 Turnstile, ordinary payments into the old Orchard pool are disabled after NU6.3 activation, so new payments to that receiver travel as Ironwood.

**Turnstile** — The ZIP 318 consensus mechanism that, from NU6.3 activation, constrains old-Orchard-pool spends to spends-plus-change and disables ordinary payments into that pool. It makes the Orchard→Ironwood migration effectively mandatory and bounds circulating supply as value crosses pools.

**Migration** — The ZIP 318 retirement of the Orchard pool into its Ironwood successor: the consensus event, enforced by the Turnstile from NU6.3, that disables ordinary payments into old-Orchard and forces its value across to Ironwood. It names a specific protocol event for one fixed pool pair (Orchard→Ironwood), not a generic movement of funds between pools — moving value between any other pools is an ordinary Send. A wallet migrates either by the private, scheduled two-phase flow (note splitting, then canonical Parts) or by the immediate, non-private Drain. *Avoid*: "migrate" for a user-chosen cross-pool sweep (that is a Send, not a Migration).

**Drain** — The immediate, non-private migration path (ZIP 318's "migrate immediately" option): spend every Orchard note worth more than the Sweep Minimum straight into Ironwood in one round of no-change transactions, accepting that the on-chain amounts are the wallet's real values and so correlate. Unlike the private path's Parts, a Drain is *transmitted* over the ordinary send connection, never over the decoupled Migration Transmission Endpoint. Its destination is fixed at Ironwood, because Migration retires exactly one pool. *Avoid*: "migrate" unqualified for this path (the private scheduled flow is equally a Migration; a Drain is specifically the immediate, non-private one).

**Sweep Minimum** — The ZIP 318 migration policy threshold (provisionally twice the ZIP-317 marginal fee). Migration never selects a note worth at most the Sweep Minimum: the policy demands a note return strictly more than a safety factor over its true marginal spend cost, not merely break even. Distinct from Dust, which is a smaller, balance-level threshold.

**Stranded** — Value a migration plan leaves behind in the Orchard pool because moving it is not worthwhile. This covers notes worth at most the Sweep Minimum, pooled balance too small to fund the smallest denomination, and balance that would arrive at or below the Sweep Minimum after fees. A plan reports its stranded value explicitly; value is never dropped silently.

**Denomination** — A canonical value permitted for a migration Part's Ironwood output, drawn from a small standard set fixed by ZIP 318 and shared by every migrating wallet, so migrated amounts collide across the whole population instead of fingerprinting a wallet. Value below the smallest Denomination cannot cross the Turnstile as a standard note. *Avoid*: "bucket" for a value quantum (the early Shielded Labs migration writing used it that way); in this repo a Bucket is a scheduling window only.

**Part** — One canonical pool-crossing migration transfer: a transaction with exactly one Orchard spend and one Ironwood output worth a single Denomination, no change output, and the canonical fee. A migration is a set of Parts, each pre-funded by an exactly-sized note so no Part waits on another's change.

**Note splitting** — Phase 1 of the private Migration: Orchard→Orchard self-sends that reshape a wallet's arbitrary Orchard notes into notes worth exactly one Denomination plus a Part fee, so each later Part spends one note with no change. Both ends are shielded, so it reveals no value and may run before NU6.3 activation; it is *transmitted* over the ordinary connection, never over the Migration Transmission Endpoint. It both merges fragments and divides large notes (see Round). *Avoid*: "splitting" for the fan-out alone.

**Round** — One group of independent Note-splitting transactions built and transmitted together. Rounds are sequential: a Round's shielded outputs must confirm and be witnessed before the next Round can spend them, because a later Round's inputs are an earlier Round's outputs. A wallet needs about log₃₂(N) Rounds and most need one. A consumer drives Phase 1 one Round per call. Distinct from a Batch, which is a group of Phase 2 Parts, and from a Bucket, which is the window a Batch transmits in; neither is a splitting step.

**Bucket** — A migration scheduling window: the span of consecutive block heights between one Boundary and the next. A Part assigned to a Bucket is transmitted while the chain tip is inside that window. A Bucket says only when a Part is sent, never what it proves against: the anchor comes from a separate draw at proving time (see Anchor age). Buckets are windows in chain time, never value quanta (see Denomination).

**Batch** — The Parts assigned to one Bucket: the group a user signs and transmits together in a single visit, since one visit while the window is open sends all of them. Batch membership falls out of the Poisson schedule's drawn transmission heights; nothing sizes Batches toward a visit target. Distinct from a Round, which is a Phase 1 splitting step, and from a Cohort, which spans wallets instead of collecting one wallet's own Parts.

**Boundary** — A block height divisible by the ratified bucket modulus. Boundaries delimit Buckets and are identical for every wallet on the network, so a migration transaction anchored at a Boundary reveals nothing about when its wallet planned or signed it. Every Part's anchor is a Boundary, but never the one that opens the Bucket it transmits in (see Anchor age).

**Anchor age** — How many Boundaries below the most recent Boundary a Part's anchor sits, counted at proving time. Drawn per Part when it proves and never zero, so a Part's anchor is always a Boundary the chain has already passed and whose Cohort has had time to accumulate before the Part proves against it. Small ages are the likeliest and the draw is capped, so anchors stay recent without ever being the newest tree state in existence.

**Cohort** — The Parts, across all wallets on the network, that prove against the same anchor Boundary. A Cohort is the anonymity set the anchor draw exists to build: the more Parts have accumulated at a Boundary, the less a Part's anchor distinguishes the wallet that sent it. *Avoid*: Cohort for one wallet's own Parts sharing a transmission window — that is a Batch.

**Note** — A shielded output belonging to the Sapling, Orchard, or Ironwood pool.

**Coin** — A transparent output (UTXO).

**Output** — General term for either a Note or a Coin. Identified by `OutputId` (txid + output index) and tagged with `PoolType`.

**Nullifier** — A one-time token derived from a Note that, when seen on-chain, proves the Note was spent.

**Outpoint** — A reference to a transparent Coin (txid + output index) used to detect transparent spends.

---

## Keys and Addresses

**Unified Key Store** — The wallet's key hierarchy. Derives keys for all supported pools from a single BIP-39 seed phrase under ZIP-32.

**Unified Address (UA)** — A single address encoding receivers for multiple pools (Orchard, Sapling, Transparent). Defined by ZIP-316.

**Receiver Selection** — The subset of pools enabled for a given Unified Address.

**Account** — A ZIP-32 account within the key store, identified by `zip32::AccountId`. A wallet may hold multiple accounts, each with its own derived `UnifiedKeyStore`. Accounts are created sequentially; the limit is the 31-bit `AccountId` ceiling (effectively unreachable).

**Spending Wallet** — A wallet created from a mnemonic phrase or Unified Spending Key. Has full spend capability: can propose transactions, calculate them, and sign them for Transmission. Returns `RecoveryInfo` on request.

**View-only Wallet** — A wallet created from a Unified Full Viewing Key. Can scan the chain, detect received funds, and report balances, but cannot sign transactions. Has no `RecoveryInfo`.

**RecoveryInfo** — The minimum data needed to restore a Spending Wallet on a new device: seed phrase, birthday, and account count. Not available for View-only Wallets.

**Transparent Address Discovery** — The process of scanning for transactions related to transparent addresses up to a configured gap limit.

---

## Wallet

**LightWallet** — The wallet's internal data store. Holds keys, wallet transactions, shard trees, and sync state. Not part of the public contract — all external access is intended to go through `LightClient` passthrough methods. Direct access via the temporary `LightClient::wallet()` escape hatch is pending removal as the passthrough API is completed.

**WalletTransaction** — An on-chain transaction containing wallet-relevant Notes, Coins, and spend data as decoded by the sync engine.

**WalletBlock** — A compact record of a scanned block, retained only at scan range bounds or when it contains wallet-relevant transactions.

**Shard Tree** — An incremental Merkle tree structure used to build note commitment tree witnesses required for spending Notes.

**Frontier** — The right edge of a note commitment tree: the newest commitment plus the minimal set of subtree roots needed to keep appending and to compute the current root. A Frontier locates the tree's end — its position is what a Checkpoint records — but proves nothing about interior commitments; witnesses come from scanned subtrees, not Frontiers. Serving a Frontier at a height is how a wallet joins the chain at its Birthday without scanning prior history.

**Checkpoint** — An association between a Block Height and the state a Shard Tree had at that height: the position of its Frontier, or emptiness. Checkpoints are the only height-indexed data in a Shard Tree; they anchor witnesses to specific heights and bound how far a Re-org can roll the tree back. Appending a checkpoint stamps the *current* Frontier at a new highest height; checkpoints for past heights must instead travel with the historical tree state they describe.

**WalletSettings** — Runtime configuration embedded in the wallet: sync config and minimum confirmations.

---

## LightClient

**LightClient** — The user-facing entry point. Owns the connection to the Indexer, manages the sync lifecycle, and exposes operations such as send, shield, rescan, and balance.

**Indexer** — Any gRPC server that serves compact blocks and transaction data to the LightClient. Abstracted behind `zingo_netutils::GrpcIndexer`. The implementation this repo targets is **zainod** (Rust, part of the zaino project); public-server deployments such as `zec.rocks:443` speak the same lightwallet gRPC protocol. This repo's test suites run zainod exclusively (the Core stack — see the test-infrastructure glossary).

**ClientConfig** — Construction-time configuration for `LightClient`: indexer URI (optional), chain type, and wallet directory.

**Indexerless** — The state of a `LightClient` that has no configured Indexer. An Indexerless client can create and restore wallets, load wallet files, read balances and history from stored state, and propose and sign transactions, yielding Calculated Transactions for later Transmission; sync, Transmission, and mempool observation require an Indexer and return `LightClientError::Offline`. The state *may* be transitional — configuring an Indexer via `set_indexer_uri()` exits it — but a consumer may equally remain Indexerless for an entire session. The default configuration starts Indexerless. See `docs/adr/0001-offline-by-default.md`. Indexerless describes the Indexer connection only: a client with a Migration Transmission Endpoint configured is Indexerless yet still emits network traffic when transmitting migration parts. *Avoid*: offline client, serverless ("Offline mode" names the CLI session concept below).

**Offline mode** — The zingo-cli session mode, committed at argument-parse time by the `--offline` flag, in which the session never configures any network endpoint — neither an Indexer nor a Migration Transmission Endpoint. The contract is zero network traffic for the life of the session, and no in-session act can lift it: the session does not offer the `network` command or any other network-requiring command, and the only exit is relaunching without `--offline` (ratified 2026-08-05). Every capability of the Indexerless state is available at the prompt. Contrast a default session, which connects to an Indexer and may still pass through the Indexerless state before connecting, and an unconsented first-boot session, which runs without an Indexer but still offers `network on` as its in-session Connectivity Consent act. *Avoid*: serverless mode, standalone wallet (retired names for this concept).

**Last Known** — The minted prefix for any zingo-cli report rendered from stored wallet state without probing the network (the Last Known height, the Last Known ranked servers). A Last Known report states its own vintage from stored data only — the mined time of the Last Known block where the wallet holds it — and never emits traffic to freshen itself. The phrase matches the sync engine's own vocabulary (`last_known_chain_height`). Ratified 2026-08-05.

**Connectivity Consent** — The user's explicit act of taking a session online, the outer tier of the wallet's two consent tiers (ratified 2026-07-28). On first boot every consumer — zingo-cli, zingo-mobile, and zingo-pc — starts offline; going online happens only by this explicit act, and the user may store the choice as a standing consent so later sessions attach to the network automatically. Connectivity Consent is therefore persistable, in deliberate contrast to the inner tier — the per-session transport consent that Mixnet Mode's switched-off state records — which is never persisted. The stored choice is fail-closed: an absent or unrecognized record withholds the connection, so corruption can keep a session offline but never take one online. The absence of Connectivity Consent is the ground state, not a preference: an Unattached Mixnet Mode or an Indexerless client implies nothing about the user's intent to go online. zingolib owns the record and its predicate; consumers render the prompt and pass the acts in (in zingo-cli: `--online`, `--remember-online`, `--forget-online`, an explicit `--server`, or the in-session `network on`, which grants for the session only; `network off` revokes for the session only, tearing down every connection without touching the stored record — see `docs/adr/0032-network-off-is-zero-emission-teardown.md`). zingo-mobile and zingo-pc adopt the stored choice in their convergence phases. The doctrine extends the offline-by-default decision to every consumer's first boot; see `docs/adr/0025-going-online-requires-connectivity-consent.md` and `docs/adr/0001-offline-by-default.md`.

**Network Observer** — The capability token a session holds exactly when it is entitled to touch the network: a network-reaching operation demands a Network Observer, and the token is constructible only at the moment connectivity is granted. An Offline session can never construct one, so the Offline mode contract — zero traffic for the life of the session — becomes a property of what code is writable rather than a promise that scattered runtime refusals keep. This is parse-don't-validate applied to connectivity: the grant is parsed once, where the Observer is minted, and never re-validated downstream. Two Observers exist, mirroring the two consent surfaces, and the split is itself the sync-only clearnet policy stated as types (ADR 0027): the **Sync Observer** is minted by Connectivity Consent plus a configured Indexer and legitimizes only synchronization, the one surface clearnet serves; the **Transmit Observer** is minted by Mixnet Mode reaching ready, or by the switched-off state's explicit clearnet consent, and every send and migration-part Transmission demands it. The price fetch demands the Transmit Observer's mixnet-ready sub-type, never its clearnet-consented form (ADR 0011). A session holding only a Sync Observer therefore cannot transmit, whatever code it runs. Ratified 2026-08-05 as the direction for the capability redesign; unimplemented — today the same contract is enforced at run time by the typed Offline refusal. *Avoid*: connection witness, network witness (retired working names); a single undifferentiated network capability (it would let clearnet serve transmit, which ADR 0027 forbids); distinct from the mixnet status subscription, which watches the transport's state rather than granting use of it.

**Migration Transmission Endpoint** — An optional network endpoint, distinct from the Indexer, to which Ironwood migration parts are submitted. A dedicated endpoint decouples part submission from synchronization so the synchronization server cannot correlate the two activities (ZIP 318). When unset, parts fall back to the Indexer with a logged correlation warning; when neither endpoint is configured, submission returns `LightClientError::Offline` and no traffic is emitted. Renamed from Migration Broadcast Endpoint 2026-08-07 (see **Broadcast**); the config-key and code sweep landed the same day. _Avoid_: Migration Broadcast Endpoint.

**WalletConfig** — Specifies how a `LightClient` should initialise its wallet. Five variants:
- `NewSeed` — generate a fresh wallet from a new random mnemonic.
- `MnemonicPhrase` — restore a wallet from an existing 24-word BIP-39 seed phrase.
- `Ufvk` — create a view-only wallet from a Unified Full Viewing Key (no spend capability).
- `Usk` — create a wallet from a Unified Spending Key.
- `Read` — load an existing wallet from the file at the configured wallet directory (see **Wallet File**).

---

## Sync

**pepper-sync** — A published crates.io library owned by ZingoLabs. Provides the sync engine for zingolib: non-linear scanning, spend-before-sync, pause/resume/stop, and fixed-memory batching. Developed inside this workspace (path dependency during development, versioned releases for consumers).

**Scan Range** — A contiguous range of block heights assigned a priority (e.g. `ChainTip`, `Historic`, `Verify`, `Scanned`).

**Fully Scanned Height** — The highest block height at or below which the wallet has completed scanning all blocks.

**Nullifier Map** — A map of all nullifiers collected during scanning, used to detect shielded spends.

**Outpoint Map** — A map of all outpoints collected during scanning, used to detect transparent spends.

**SyncMode** — The current state of the sync engine: `NotRunning`, `Running`, `Paused`, or `Shutdown`.

**SyncPauseGuard** — A guard that holds the sync engine paused — actively paused or not running — for as long as the value lives. `LightClient::pause_sync_scoped` is the sole constructor: it pauses a running engine, and dropping the guard restores the prior sync mode. The Orchard→Ironwood drain demands the guard as a parameter, so planning under a running sync is unrepresentable for that path; the Two-phase Send holds one internally beside the stored Proposal, giving the shipped propose/send protocol the same guarantee without a signature change.

**SyncConfig** — Configuration for the sync engine: performance level and transparent address discovery settings.

**Performance Level** — Controls batch size and nullifier map scope: `Low`, `Medium`, `High`, or `Maximum`.

---

## Send Flow

**Proposal** — A pre-computed spend plan produced by `LightWallet::create_send_proposal`. Stores the selected inputs and outputs but does not yet build the transaction. Exposed intentionally so callers can inspect fees before committing.

**ZingoProposal** — A proposal paired with the account it spends from, stored in the wallet between proposal creation and transaction calculation while the user decides whether to accept the fee. Consumed (removed from the wallet) only when `send_stored_proposal` reaches its calculation step or `calculate_stored_proposal` signs offline; an Indexerless send attempt fails typed before consuming it, so it survives for retry once an Indexer is configured. The slot is process-lifetime state: wallet reads reset it (see the proposal-persistence non-goal in ADR 0006). See ADR 0006.

**ConfirmationStatus** — The lifecycle state of a transaction. Ordered for sorting (confirmed first): `Confirmed(height)` → `Mempool(target_height)` → `Transmitted(target_height)` → `Calculated(target_height)` → `Failed(height)`. For non-confirmed states the embedded height is the chain height at time of creation + 1 (the intended target block), not an actual confirmation height. `Failed` is a permanent terminal state — failed transactions remain in the wallet as a record. Consumers filter them out at the display layer using `ConfirmationStatus`.

**Calculated Transaction** — A fully built and signed but not yet transmitted transaction. Status: `Calculated`. One calculated while Indexerless has its expiry retargeted to the last height of the current consensus epoch: it remains transmittable until the next scheduled network upgrade, the outer limit for any pre-signed Zcash transaction. See `docs/adr/0008-offline-expiry-by-retarget.md`.

**Transmission** — The step in which the client attempts a send: it submits a Calculated Transaction to the Indexer and verifies that the server-reported txid matches the locally calculated txid. Ironwood migration parts travel a distinct submission path — the **Migration Transmission Endpoint**, with no txid echo check — but the path is named with the same word: both are targeted submissions to one Correspondent. The verb is *transmit*, the noun *Transmission* — a grammatically complete pair, where "broadcast" served both roles in one ambiguous form. Ratified 2026-08-07, deliberately breaking with ZIP 318's word "broadcast" for part submission; see **Broadcast**.

**Broadcast** — Reserved for genuinely many-recipient delivery, per the networking convention the word's "broad" announces. No submission path in this wallet broadcasts: every path discloses to one drawn Correspondent on the happy path, and even failure escalation contacts a bounded few, winner-take-all. Ratified 2026-08-07; the code and docs sweep retiring the old uses landed the same day. _Avoid_: broadcast for any targeted submission (the ZIP 318 sense is inherited spec language, departed from deliberately).

**Two-phase Send** — The standard send path: `propose_send` (or `propose_shield`) followed by `send_stored_proposal`. Allows the caller to inspect the Proposal (e.g. fees) before committing. Proposing pauses the sync engine and the client holds that pause (a **SyncPauseGuard**) while the Proposal is stored, so the state proposed against cannot shift before the send builds it. The pause ends with the Proposal: `send_stored_proposal(resume_sync: true)` and `clear_proposal` (the decline path) restore the engine's prior mode, `resume_sync: false` leaves it paused for the caller, and a proposing call that fails restores the engine on its way out. `quick_send` / `quick_shield` are single-shot convenience wrappers that hold the pause for the span of one call. The pause's outside view is the Privacy section's **Quiescence Signature**, which today carries send timing; see that entry for the de-correlation requirement.

**Narration** — Human-facing progress reporting emitted while a long-running operation is in flight: the transmit path's latest progress line, a migration batch's build/send counts, server probing. Narration is presentation, never data: it carries no parse contract, its wording may change in any release, and in zingo-cli it travels only on stderr, because stdout carries exactly the command's result (see `docs/adr/0031`). The library publishes narration pull-style through progress handles; each frontend decides its own cadence and rendering. *Avoid*: parsing narration lines (typed frontends poll the progress handles; machine consumers read the stdout result).

**Transmit Heartbeat** — zingo-cli's liveness tick during a Transmission, migration-part transmission, drain, or split: while the operation blocks the session, the heartbeat prints the latest Narration line with elapsed time at a fixed cadence, proving the process is alive so a user does not kill a slow mixnet send mid-transmission. An operation that finishes before the first tick stays silent, and a tick with no progress line yet available still fires with a generic line: the contract is liveness first, Narration when it exists.

---

## Balance

**AccountBalance** — A per-account snapshot of spendable funds, broken down by pool (Orchard, Ironwood, Sapling, Transparent) and confirmation state (confirmed, unconfirmed, total). `None` for a pool means the account has no view key for that pool. Dust outputs are excluded from all figures. Coinbase transparent outputs require 100 confirmations before appearing as spendable (ZIP-213 `COINBASE_MATURITY`).

**Zatoshis** — The atomic unit of ZEC. 1 ZEC = 100,000,000 zatoshis.

**Dust** — Outputs whose value is strictly below the ZIP-317 marginal fee. Excluded from balances and not eligible as inputs. Distinct from Stranded value: the migration Sweep Minimum sits above the Dust threshold, so a note can be spendable in the ordinary send path yet still be Stranded by migration.

---

## Transparent Protocol Extensions

**TEX Transaction** — A Transparent-Exposed Transaction (ZIP-230). A two-step flow that moves shielded funds to a publicly visible transparent address by routing through an ephemeral Refund Address first.

**Refund Address** — An ephemeral transparent address (`TransparentScope::Refund`) generated by the wallet for the first step of a TEX transaction. Not intended for direct use by senders; used internally to bridge the shielded-to-transparent path.

---

## Summary / Display

**Perspective** — The library's single editorial reading of wallet history: the layer that derives presentation-facing statements (ValueTransfers, rollup totals) from the canonical record. The singular is load-bearing — one house perspective, many renderers: consumers render its statements and never re-derive them, and its outputs never feed wallet logic. Distinct from the canonical record it reads (TransactionSummary and the wallet's own state). *Avoid*: viewmodel (invites an MVVM misreading), opinions.

**TransactionSummary** — A snapshot of a single wallet transaction for display purposes. Not used for internal wallet logic.

**ValueTransfer** — A single directional movement of funds within a transaction: received, sent, shielded, or self-sent.

**TransactionSummaries / ValueTransfers** — Ordered collections of their respective types.

---

## Donation

**Zennies for Zingo** — An opt-in donation feature. The library exposes hardcoded addresses (`ZENNIES_FOR_ZINGO_DONATION_ADDRESS`, per chain type) and a suggested amount (1,000,000 zatoshis = 0.01 ZEC). Callers include this address as a recipient when constructing a send request to donate. Nothing is added automatically.

**Developer Donation Address** — A separate hardcoded address for developer donations, also opt-in.

---

## Privacy

**Tor** — A former opt-in privacy layer (`zcash_client_backend`'s `tor::Client`), only ever wired to price fetching, never to sync or Transmission. Removed in June 2026 over dependency conflicts. Not the project's privacy direction; IP obfuscation is taken up on the Nym mixnet instead. See `docs/adr/0011-nym-mixnet-transmission.md`.

**Nym mixnet** — The mix network chosen as the privacy transport for IP obfuscation: it hides the client's IP from the services it contacts by routing traffic through a multi-layer network, so a service sees only a mixnet exit, never the client. Carries the send (Transmission) and price-fetch surfaces. Distinct from **NymVPN**, Nym's lower-latency VPN product, which is an acceptable but user-provided (system-level, not embedded) transport for the sync surface. See `docs/adr/0011-nym-mixnet-transmission.md`.

**Lockfile Split** — The build invariant that the Nym mixnet stack and the wallet's Zcash cryptography stack never share one `Cargo.lock`: their `crypto-common` requirements cannot be reconciled in a single resolution. `zingo-netutils` therefore stands outside the workspace as its own resolution unit — the workspace consumes it with the mixnet feature off, while the proxy builds from netutils' own lockfile with it on. It follows that the wallet library and the Nym proxy are always two artifacts on every platform, meeting only at a runtime SOCKS5 boundary; the mixnet stack can never ride inside the wallet binary, whatever the packaging. *Avoid*: "disjoint dependency graphs" (the crates and most of the graph are shared; only the lockfile resolutions are disjoint). See `docs/adr/0011-nym-mixnet-transmission.md`.

**Mixnet Mode** — The runtime state governing whether Transmission, price-fetch, and sync of the Mixnet Sync Window route over the Nym mixnet. Five states: unattached, switched off, bootstrapping, ready, and died. Unattached is a present condition, not a history claim: no mixnet transport is established and no consent to clearnet has been recorded, so the mixnet surfaces refuse, exactly as they do while bootstrapping or died — the absence of a transport is never consent. It is the initial state, and equally the state after a failed enable or re-enable, even when a transport ran earlier in the session. *Avoid*: never-attached (a wallet returns to unattached when a re-enable fails). Switched off is the user's deliberate per-session toggle-off and the one state that routes those surfaces over clearnet, as informed consent; it is reached only by that explicit act, never by default, by failure, or by a transport's absence. The explicit act may be performed at session start: an opt-out the user submits with the session's launch records the same consent as an in-session toggle-off. Bootstrapping is enabled-but-not-yet-reachable, and ready is reachable. Died is an unexpected loss of the spawned proxy — it exited during bootstrap or after reaching ready — and is distinct from switched off precisely because it is unconsented: a died proxy makes the surfaces refuse rather than fall back to clearnet, and the user recovers by re-enabling the mixnet, which spawns a fresh proxy. The extra states beyond a bare boolean exist because a client not presently protected by the mixnet must neither transmit over clearnet without consent nor report itself protected — and because "no transport" and "consented clearnet" are different facts that must never share a representation. On by default for any connected session and never persisted; while the mode is on, a transport failure refuses a send rather than silently dropping to clearnet. An `--offline` session, which never transmits, never bootstraps the mixnet. The proxy's lifetime is bound to the wallet session: a terminal interrupt cannot kill it out from under a live session, and no orphaned proxy survives its parent. *Avoid*: off (the retired name for switched off — retired because the implementation once reported it for a never-attached wallet too, conflating consent with absence). zingo-cli no longer offers an act that reaches switched off: its `network off` names the connectivity teardown, not the mixnet toggle, so the state remains reachable only by consumers that still offer a toggle-off act (see `docs/adr/0032-network-off-is-zero-emission-teardown.md`). See `docs/adr/0011-nym-mixnet-transmission.md`.

**Correspondent Selection** — The draw that decides which Correspondent receives a Transmission: random, under two constraints. The sync indexer's operator is never eligible (see Correspondent Indexer), and the Correspondent must differ from the previous Transmission's, remembered for the session only and never persisted. The property this buys is non-accumulation: no single indexer collects a record of the user's consecutive sends. Selection is not rotation — nothing is consumed or replaced by a draw; contrast Exit Rotation. Ratified 2026-07-28; renamed from Witness Selection 2026-08-07. _Avoid_: Witness Selection.

**Correspondent** — The indexer a Transmission is submitted to: the served party that receives the transaction, relays or suppresses it, and thereby learns it. Replaces "witness" in the send-privacy vocabulary (ratified 2026-08-07), because a witness is by definition not a party, while the Correspondent is a principal — served, acting, and held to account by the delivery check. The term is deliberately idiosyncratic, taken from banking's correspondent — the institution that acts on another's behalf at a remove — rather than from the cited anonymity literature, whose candidates all fail: "recipient" and "service provider" name topology without the accounting role, "counterparty" collides with a payment's payee, "observer" is taken by the Network Observer. Bare "witness" remains the Merkle authentication path inside sync code, and only there. _Avoid_: witness (for this role), broadcast witness, observer.

**Correspondent Rotation** — The send-privacy property governing a single Transmission's escalation: on the happy path exactly one Correspondent receives the transaction, and when delivery cannot be confirmed the Transmission escalates through serially gated rounds of fresh draws (each drawn by Correspondent Selection), accepting redundancy only on that failure path, up to a fixed cap of distinct Correspondents. This escalation guards against a censoring Correspondent that accepts a submission but suppresses or misreports the relay. Renamed from Witness Rotation 2026-08-07; the code and docs sweep landed the same day. _Avoid_: Witness Rotation, Witness Selection (now Correspondent Selection). See `docs/adr/0011-nym-mixnet-transmission.md`.

**Exit Rotation** — The send-privacy property whereby consecutive Transmissions never share a mixnet exit, so the exit never becomes a linking key at the destination. The wallet keeps a standing pool of two ready transports; a Transmission consumes one, and a replacement is brought up in the background, so rotation never costs a send the cold-tunnel wait. A cold pool never lends a send another surface's tunnel: the send waits on its own PrioritisePrivacy acquisition (ruled 2026-08-08). Complements Correspondent Selection on the other side of the tunnel: the Correspondent differs per send, and so does the exit that delivers to it. Ratified 2026-07-28; mechanism not yet implemented.

**Indexer Operator** — The party in administrative control of an indexer endpoint, derived as the endpoint host's registrable domain: one domain is one administrative authority, so endpoints sharing a domain share an operator. The derivation is sound in that direction only — distinct domains are merely the absence of evidence of common control, never proof of it — and policies that need operator disjointness lean on that weaker direction; where a pairing is privacy-critical, distinctness deserves recorded attestation, and infrastructure correlations (a shared custom build, a shared certificate chain) count against it. Ratified 2026-08-04. See `docs/adr/0029-a-maintained-mixnet-indexer-pool-replaces-server-selection.md`.

**Maintained Indexer Pool** — The standing set of mixnet connections the wallet keeps to every eligible census endpoint, supervised for the lifetime of the client: a dead connection is re-established, never ranked or replaced by preference. The pool has no winner — membership is defined by liveness alone — and it retires server selection as a concept: latency ranking is meaningless through a mixnet, and mixnet fan-out reveals no client IP, so contacting everyone costs nothing the old clearnet census probe used to charge. The pool carries control traffic, the send escalation, Correspondent duties, and price — never bulk sync, which stays on its ratified route. Ratified 2026-08-04. See `docs/adr/0029-a-maintained-mixnet-indexer-pool-replaces-server-selection.md`.

**Exit Budget** — The per-platform count of standing mixnet transports a client affords, governed by the budgeted exit-diversity invariant: within the budget, no exit carries connections to more than one Indexer Operator. Full width is one exit per endpoint, uniformly sampled without replacement from the exit-provider population — at which width the invariant holds by construction and no exit observes more than one connection at all. A platform that cannot afford full width partitions endpoints across its smaller budget, never crossing operators within an exit, accepting the residual linkage inside each partition rather than pretending it away. Exits are drawn from distinct nym-node runners as far as the directory allows, so exit-side diversity is not quietly reconstituted into a single observer. Ratified 2026-08-04. See `docs/adr/0029-a-maintained-mixnet-indexer-pool-replaces-server-selection.md`.

**Exit Node** — The mixnet egress a transport binds: the exit gateway together with its network requester, named by its identity. The proxy announces each bound Exit Node to the wallet, `network on` names them in its success report, and the Exit Rotation design holds two. *Avoid*: exit provider (a code-level field name), gateway (nym-internal vocabulary). Ratified 2026-08-06.

**Exclusive Exit Node** — An Exit Node its holder uses toward exactly one Correspondent for the whole life of its Exclusive Lease, so the exit can link the client to at most one destination. A Transmission's connection is the paradigm case. The category is a property of the acquiring operation, never of the node: any node in the Exit Pool can serve either category, the operation declares the category at acquisition, and the lease carries it, enforced in the type system where feasible. Orthogonal to the Responsiveness Class, which governs the acquisition's widening, not what the bound exit may observe. Ratified 2026-08-07. See Shared Exit Node, Exclusive Lease.

**Shared Exit Node** — An Exit Node its one holder uses toward many Correspondents — the Server-Selection Sweep and the price fetch are the paradigm cases — so the exit necessarily observes the holder's fan-out set. Shared means shared among Correspondents within a single holder, never between holders: the Exit Node Reservation's unique-per-holder invariant stands, and a Shared Exit Node is held under an Exclusive Lease exactly like any other. Ratified 2026-08-07. _Avoid_: reading Shared as two transports on one exit (that reading contradicts the Exit Node Reservation invariant).

**Server-Selection Sweep** — The mixnet-only survey that establishes which indexers are live: one `GetLightdInfo` per candidate, carried entirely over the mixnet, never over clearnet. A candidate is live when its reported chain matches the session's chain and its reported height sits within two blocks of the sweep's observed median height — the median, never the maximum, so a single inflated report cannot capture selection. The sweep's candidates are the census's active mixnet-eligible entries for the session's chain, together with any candidates the user supplies. The live cohort feeds the sync-attach draw (one ticket per Indexer Operator); the height-descending order is the failover sequence and the report order, never the selector. An explicit user pin bypasses the draw, never the sweep: the pinned server is surveyed like any candidate and judged against the same cohort median, it is selected when live, and when it is not live the session runs offline and names the dead pin as the reason, never falling back to an unpinned choice; the posture is liftable, and the in-session consent act retries the same pin. An empty cohort ends the Sync Session with a typed refusal that names every candidate's failure, and leaves the session's posture untouched: a pin binds the session, an unpinned sweep binds only its Sync Session. The sweep rides its own transport under its own Exclusive Lease, so its Exit Node is never one that send or price-fetch holds while the sweep runs; the lease recycles into the Exit Pool when the sweep completes. The selected sync indexer is excluded from the live candidates that serve the transmit operations. Ratified 2026-08-06.

**Exit Pool** — The session's population of eligible Exit Nodes, discovered once and held for the session. Every transport that needs an egress draws Exit Node Reservations from it, and the pool is the sole issuer: it holds one reservation per node and never issues a held reservation twice. Exclusion follows use — a node is ineligible exactly while some transport holds its reservation — with one evidence-based exception: each node carries a per-session count of its connection failures, and a node whose failure rate stands out against the pool's is withheld for the rest of the session (see Session Retirement). Concurrent transports are disjoint by construction, because two holders cannot hold the same reservation. Ratified 2026-08-07.

**Exit Node Reservation** — The claim on one Exit Node that a transport draws from the Exit Pool. The governing invariant is that an Exit Node Reservation is unique to its holder: the pool issues one reservation per node, a draw transfers it, and no two holders ever hold a reservation for the same node. A transport draws a Clutch of Exit Node Reservations at acquisition and races its connection over their nodes; the reservation whose node it binds becomes its Exclusive Lease, and every other reservation MUST recycle the moment the lease exists — reserved-but-unused reservations are never held past the bind, so the pool drains only by what is actually leased. The draw is uniform over the pool's unissued reservations, never assigned by preference. Disjointness among live transports follows from the uniqueness invariant alone, so no transport need learn what any other bound. *Avoid*: Exit Node Reservation Token, ENRT (the retired working name). Ratified 2026-08-07.

**Exclusive Lease** — The one Exit Node Reservation a transport keeps after binding its Exit Node: the reservation for the bound node, held for the rest of the transport's lifecycle and recycled when that lifecycle ends. A transport holds many reservations only during its acquisition race and exactly one Exclusive Lease after it; a post-bind failure therefore ends the lifecycle — lease recycled, fresh draw, fresh transport — rather than redrawing within a set the bind already returned. Ratified 2026-08-07.

**Responsiveness Class** — The compile-time partition of network operations into two categories, each carrying its own acquisition policy. The class is a property of the operation alone; the acquisitions, races, and proxy instances serving an operation inherit its class unchanged, and no downstream artifact carries a class of its own — in particular a pooled connection has no class, since a later operation of either class may ride it. A **PrioritiseSpeed** operation is one where latency governs the widening and racing wide carries no privacy cost — the session's go-online act, the Server-Selection Sweep gating a Sync Session, `network on` at the prompt — and its acquisition saturates the race: the full width launches at once, buying minimum latency at maximum exposure. A **PrioritisePrivacy** operation is one where parsimony — privacy or load — outranks latency, and its acquisition hedges lazily: one candidate, another only after a quiet interval or a failure, buying minimum exposure at the cost of patience — the standing pool's background refills, and equally a send that finds the pool cold and elects to wait rather than widen. Every acquisition site names its class in the type system, the partition is sealed so no third class can be minted, and the class crosses the process seam to the spawned proxy as a wire token. Ratified 2026-08-07; axis restated the same day — the class names the widening's governing concern, never whether a user waits, since a user also awaits a send, which hedges. Renamed from Critical and NonCritical 2026-08-08 so the names state the axis. _Avoid_: Critical, NonCritical; defining the classes by whether a user waits.

**Clutch** — The group of Exit Node Reservations one transport draws in a single acquisition: a uniform random sample from the Exit Pool's unissued reservations, of size three. The clutch is the unit of acquisition — the transport races its connection over the clutch's nodes, binds one into its Exclusive Lease, and recycles the rest — and equally the unit of retry: a transport that exhausts its clutch dies, and the parent recycles the spent clutch and draws a fresh one for the respawn. The clutch is the design's only three-wide quantity: a race's width ceiling is the clutch size, never an independent parameter. Ratified 2026-08-07.

**Acquisition Race** — The winner-take-all redundancy pattern of an acquisition: the clutch's reservations are the arms, a connect attempt is one pull of an arm, the class's launch policy schedules the pulls, the first success binds, and every loser is cancelled. Today each arm is pulled at most once. The vocabulary is the source literature's, exactly — arm and pull from the multi-armed bandit (Robbins 1952), racing from Happy Eyeballs (RFC 6555, RFC 8305), hedging from the tail-at-scale hedged request (Dean & Barroso 2013) — and hedging names a launch policy within a race, never the race itself. Code spells the term `acq_race` (`AcqRace` in type names), so identifiers name the Acquisition Race explicitly. Ratified 2026-08-07. See `docs/adr/0035-the-acquisition-race-speaks-the-literatures-vocabulary.md`. _Avoid_: bare "race" for an acquisition's race in code identifiers.

**Session Retirement** — The session-scoped, evidence-based withdrawal of an Exit Node from the Exit Pool. Every node carries a per-session counter of the times it failed to support a connection; when a node's failure rate stands more than one standard deviation above the pool's mean, its reservation is withheld from every later draw for the rest of the session. Retirement is the deliberate exception to exclusion-follows-use: the one mark a node's history leaves, scoped to the session and never persisted. Distinct from Exit Recycling, which returns a reservation after use — recycling replenishes, retirement withholds. Ratified 2026-08-07.

**Exit Recycling** — The return of Exit Node Reservations to the Exit Pool: the reserved-but-unused reservations the moment their transport obtains its Exclusive Lease, and the lease itself when the transport's lifecycle ends. Recycling is the pool's replenishment, not a retirement: no node is ever permanently withdrawn, and a recycled node is as eligible for the next draw as any other unless Session Retirement has withheld it on failure evidence. The privacy claim recycling supports is therefore probabilistic rather than absolute — a later transport may draw the very node that carried an earlier sweep, at roughly the population's re-draw rate — and the guarantee it does buy is exact for concurrency: while a transport holds a reservation, no other transport can bind that node. *Avoid*: recycling for the discard or retirement of an exit (the retired 2026-08-06 sense, which read the word as its own opposite). Ratified 2026-08-07.

**Mixnet Sync Window** — The top segment of the chain whose sync always rides the Nym mixnet, governed by one per-request axiom with two request classes: the mixnet carries every sync request that names the wallet or touches the window, and the session route carries only deep, wallet-neutral chain data. Wallet-naming requests — full-transaction fetches by txid, transparent-address queries, utxo metadata, and the mempool stream — ride the mixnet at any height, because their sensitive subject is the wallet, not the chain. Height-neutral structural requests — compact-block and nullifier ranges, subtree roots, frontiers, tip queries — ride the mixnet when their subject lies above (tip − window size) and otherwise ride the session's existing route: the user's system-level NymVPN when one runs, clearnet otherwise; the wallet never embeds a dVPN. Corollaries, not separate rules: a catch-up that fits inside the window touches no clearnet at all, and the tip query is itself windowed, so a mixnet-capable session's first contact is always the mixnet. Windowed and wallet-naming requests fail closed: while the mixnet is bootstrapping they wait, if it dies they refuse typed, and they never fall back to clearnet — while below-window structural requests, already consented to the session route, proceed in parallel with bootstrap. The window's size is the Five-Minute Calibration (below), currently 30,000 blocks (about twenty-six days at the 75-second target spacing). Ratified 2026-07-28 at 7,000 blocks; re-founded 2026-08-06 on the per-request axiom, the request classes, and the calibration. See `docs/adr/0023-top-window-sync-rides-the-mixnet.md`.

**Five-Minute Calibration** — The definition of the Mixnet Sync Window's size: the block count empirically downloadable over the mixnet in five minutes, the mobile background-sync slot. The project measures it and ratifies the result as a named constant per release; it is never measured per device or per session, so every wallet shares one boundary height. A slow device takes longer than five minutes rather than shrinking its window: the calibration bounds the typical catch-up's duration, never the privacy guarantee. Ratified 2026-08-06.

**Sync Session** — One sync run, from start to quiescence. The unit of sync freshness: each Sync Session gets a dedicated mixnet tunnel with its own fresh exit (never drawn from Exit Rotation's send pool) and a freshly selected sync indexer. A Transmission can therefore never share a transport, an exit, or a session with sync. Ratified 2026-07-28. Selection follows the sync-attach rule (ratified 2026-08-05): a uniform random draw over the live Indexer Operators — one ticket per operator, any live endpoint of the winner — sticky for the session and overridden only by an explicit user pin. Liveness comes from the Server-Selection Sweep that opens every Sync Session (ratified 2026-08-06); the Maintained Indexer Pool, when it lands, becomes the sweep's maintained successor. See `docs/adr/0029-a-maintained-mixnet-indexer-pool-replaces-server-selection.md`.

**Quiescence Signature** — The externally observable pattern of the wallet's sync traffic stopping and resuming, as seen by the sync indexer or any on-path observer of the sync route. Distinct from the SyncPauseGuard mechanism that produces a pause: the guard is wallet-internal state serving proposal consistency, while the signature is what the pause looks like from outside. Today the Two-phase Send's pause brackets each Transmission — sync halts at propose and resumes when the transmission completes — so the signature carries send timing, and an observer who correlates a resume with a transaction's mempool arrival links the wallet's sync identity to a transaction the mixnet was supposed to unlink. The de-correlation requirement follows: a Transmission must leave no imprint on the Quiescence Signature. Ratified 2026-08-03; remediation unshaped. See zingolabs/zingolib issue #2615.

**Sync Indexer Selection** — The draw that decides which indexer serves a Sync Session: fresh per session from the curated sync list, and never the same operator as any Transmission Correspondent, in both directions (Correspondent draws exclude the sync operator per `docs/adr/0022`, and the sync draw excludes the session's Correspondents). Applies to shielded compact-block sync only: transparent address queries reveal the address set to their indexer, so they keep a sticky route, decided separately. Ratified 2026-07-28.

**Correspondent Indexer** — An Indexer used only as a Transmission target, drawn at random from a curated Correspondent list kept separate from the sync-server list. The list holds one endpoint per operator, since the accumulating party Correspondent Selection defends against is the operator, not the DNS name. Distinct from the sync Indexer that serves compact blocks, and the distinction is an enforced invariant rather than a tendency: every transmission draw excludes the sync indexer's operator from the pool, so the address-knowing sync indexer never receives a Transmission, and a draw with no eligible Correspondent refuses rather than falling back. Renamed from Broadcast Indexer 2026-08-07. _Avoid_: Broadcast Indexer. See `docs/adr/0022-broadcast-witness-never-the-sync-indexer.md`.

---

## Persistence

**Wallet File** — The serialized wallet state, stored as `zingo-wallet.dat` inside the wallet directory. zingolib owns all file I/O: consumers provide a directory path and zingolib opens and writes the file itself.

**Save Flow** — Wallet state changes set an internal dirty flag. The `LightClient` save task wakes periodically, detects the dirty flag, serialises the wallet, and atomically writes to disk (temp file → rename, power-safe). Consumers that need an immediate one-shot flush call `LightClient::flush()`.

**Dirty Flag** — An internal boolean on `LightWallet` that records whether unsaved changes exist. Set automatically by all mutating operations; cleared after a successful write. Exposed to external code via `LightWallet::mark_dirty()`.

**Wallet Version** — The number leading a Wallet File that identifies its layout. A Wallet Version, once used by any revision that reached users, is burned: it is never reassigned to a different layout. (Version 43 is burned; version 42 names two layouts that the reader tells apart.)

**Shipped Format** — A Wallet File layout that has landed in dev. Landing in dev is the shipping event — there is no later release gate protecting dev builders — and every Shipped Format remains readable, and the wallet writable, forever after. See ADR 0015.

**Recovery Salvage** — The last-resort read that recovers the seed phrase, birthday, and account count from the stable prefix of a Wallet File whose full parse fails. A backstop beneath the Shipped Format guarantee, not a substitute for it: restoring from seed forfeits local transaction metadata and forces a rescan.

---

## Consumers

**zingolib** is a Rust library. Its primary integration surface is the `LightClient` API, consumed directly by other Rust crates and programs. One known consumer is **zingo-mobile**, which wraps `zingolib` via a UniFFI-generated FFI layer (Kotlin/Swift). `zingo-cli` is a power-user/developer CLI built on the same library.

**Reference Consumer** — A consumer whose charter is to prove zingolib's consumer surface sufficient, not to serve users: it holds no funds and makes no product promises, and its own code is confined to a typed one-to-one projection of the surface, a provisioning adapter, and a renderer — no wallet logic, no policy, no minted strings. It builds against the workspace at HEAD so that a surface change which breaks the consumer contract fails in the merging pull request's CI rather than weeks later in another repo, and it is for that reason the one consumer exempt from ADR 0024's rev-pinning rule, which disciplines external consumers. The first Reference Consumer is the planned `zingo-tauri` desktop app. Ratified 2026-08-03. See `docs/adr/0028-the-reference-consumer-lives-in-repo-in-an-excluded-sub-workspace.md`.

---

## Testing

**libtonode-tests** — Integration tests that run zingolib against a real local node stack (library-to-node). Uses `zcash_local_net` to spin up a `Validator` + `Indexer` pair; the pair is selected at compile time (the test-infrastructure glossary's "Network combo"). The only combo is `zainod+zebrad`; the legacy validator and indexer combos were removed in July 2026, the latter with the darkside-tests retirement.

**darkside-tests** — Deterministic reorg and edge-case tests that inject arbitrary blocks without mining. Their authoritative home is the mock-indexer darkside module inside zingolib, which runs offline; the standalone `darkside-tests` crate (driving the legacy indexer's "darkside" mode) is retired by the package-simplification work, its nine tests having been long ignored.

**Validator** — The consensus node in a test local net: `zebrad`.

**DefaultValidator / DefaultIndexer** — Type aliases in `zingolib_testutils` that resolve to the active backend combo at compile time, allowing test code to remain backend-agnostic.

---

## Utility Crates

**zingo-memo** — Utilities for creating and parsing the Zcash memo field.

**zingo-price** — Price feed integration (Gemini exchange).

**zingo-status** — `ConfirmationStatus` type: the confirmation state of a transaction (calculated, transmitted, mempool, confirmed, etc.).

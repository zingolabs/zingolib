# zingolib Domain Glossary

> This file is a pure glossary. No implementation details, no specs, no plans.

---

## Chain

**ChainType** — The network a wallet connects to: `Mainnet`, `Testnet`, or `Regtest`. `Regtest` carries `ActivationHeights` to configure protocol upgrade heights locally.

**Block Height** — A `u32`-compatible integer identifying a block's position on the chain. Used as the primary ordering key for sync progress.

**Birthday** — The earliest block height at which a wallet may have received funds. Scanning begins here on first sync.

**Library Birthday** — A per-ChainType block height baked into each zingolib release, chosen so that it had already been mined when the release was cut. Any wallet a given release creates necessarily post-dates the release, so this height is a safe Birthday for a NewSeed wallet created while Indexerless, where no Indexer can report the chain height. It never applies to restores: a restored seed or viewing key may predate the library, so restoring always requires a caller-supplied Birthday. The caller passes it explicitly; the library only publishes the value.

**Chain Tip** — The range of blocks at the top of the blockchain starting from the lowest block containing the last note commitment to the most recent shard, up to the current chain height. Distinct from "chain height" (highest known block).

**Re-org** — A chain reorganisation. The sync engine detects this by checking block hash continuity in a verification window at the top of the locally known chain.

---

## Pools and Outputs

**Pool** — A shielding protocol or address type. Three pools exist: `Transparent`, `Sapling`, and `Orchard`.

**Note** — A shielded output belonging to either the Sapling or Orchard pool.

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

**WalletSettings** — Runtime configuration embedded in the wallet: sync config and minimum confirmations.

---

## LightClient

**LightClient** — The user-facing entry point. Owns the connection to the Indexer, manages the sync lifecycle, and exposes operations such as send, shield, rescan, and balance.

**Indexer** — Any gRPC server that serves compact blocks and transaction data to the LightClient. Abstracted behind `zingo_netutils::GrpcIndexer`. Two implementations exist: **lightwalletd** (Go, the longstanding public-server deployment, e.g. `zec.rocks:443`) and **zainod** (Rust, part of the zaino project). The ecosystem is migrating from lightwalletd to zainod; this repo's test suites already default to zainod (the Core stack — see the test-infrastructure glossary), while lightwalletd survives in one opt-in legacy combo.

**ClientConfig** — Construction-time configuration for `LightClient`: indexer URI (optional), chain type, and wallet directory.

**Indexerless** — The state of a `LightClient` that has no configured Indexer. An Indexerless client can create and restore wallets, load wallet files, read balances and history from stored state, and propose and sign transactions, yielding Calculated Transactions for later Transmission; sync, Transmission, and mempool observation require an Indexer and return `LightClientError::Offline`. The state *may* be transitional — configuring an Indexer via `set_indexer_uri()` exits it — but a consumer may equally remain Indexerless for an entire session. The default configuration starts Indexerless. See `docs/adr/0001-offline-by-default.md`. *Avoid*: offline client, serverless ("Offline mode" names the CLI session concept below).

**Offline mode** — The zingo-cli session mode, committed at argument-parse time, in which the session never configures an Indexer: the client remains Indexerless for the life of the session, and every capability of that state is available at the prompt. Contrast a default session, which connects to an Indexer and may still pass through the Indexerless state before connecting. *Avoid*: serverless mode, standalone wallet (retired names for this concept).

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

**SyncConfig** — Configuration for the sync engine: performance level and transparent address discovery settings.

**Performance Level** — Controls batch size and nullifier map scope: `Low`, `Medium`, `High`, or `Maximum`.

---

## Send Flow

**Proposal** — A pre-computed spend plan produced by `LightWallet::create_send_proposal`. Stores the selected inputs and outputs but does not yet build the transaction. Exposed intentionally so callers can inspect fees before committing.

**ZingoProposal** — A proposal paired with the account it spends from, stored in the wallet between proposal creation and transaction calculation while the user decides whether to accept the fee. Consumed (removed from the wallet) only when `send_stored_proposal` reaches its calculation step or `calculate_stored_proposal` signs offline; an Indexerless send attempt fails typed before consuming it, so it survives for retry once an Indexer is configured. The slot is process-lifetime state: wallet reads reset it (see the proposal-persistence non-goal in ADR 0006). See ADR 0006.

**ConfirmationStatus** — The lifecycle state of a transaction. Ordered for sorting (confirmed first): `Confirmed(height)` → `Mempool(target_height)` → `Transmitted(target_height)` → `Calculated(target_height)` → `Failed(height)`. For non-confirmed states the embedded height is the chain height at time of creation + 1 (the intended target block), not an actual confirmation height. `Failed` is a permanent terminal state — failed transactions remain in the wallet as a record. Consumers filter them out at the display layer using `ConfirmationStatus`.

**Calculated Transaction** — A fully built and signed but not yet transmitted transaction. Status: `Calculated`. One calculated while Indexerless has its expiry retargeted to the last height of the current consensus epoch: it remains transmittable until the next scheduled network upgrade, the outer limit for any pre-signed Zcash transaction. See `docs/adr/0008-offline-expiry-by-retarget.md`.

**Transmission** — The step in which the client attempts a send: it submits a Calculated Transaction to the Indexer and verifies that the server-reported txid matches the locally calculated txid. *Avoid*: broadcast (reserved for a distinct future concept — a memo that multiple recipients can decrypt).

**Two-phase Send** — The standard send path: `propose_send` (or `propose_shield`) followed by `send_stored_proposal`. Allows the caller to inspect the Proposal (e.g. fees) before committing. Proposing automatically pauses the sync engine to release the wallet write lock; `send_stored_proposal(resume_sync: true)` resumes it after transmission. `quick_send` / `quick_shield` are single-shot convenience wrappers that handle the pause/resume cycle internally.

---

## Balance

**AccountBalance** — A per-account snapshot of spendable funds, broken down by pool (Orchard, Sapling, Transparent) and confirmation state (confirmed, unconfirmed, total). `None` for a pool means the account has no view key for that pool. Dust outputs are excluded from all figures. Coinbase transparent outputs require 100 confirmations before appearing as spendable (ZIP-213 `COINBASE_MATURITY`).

**Zatoshis** — The atomic unit of ZEC. 1 ZEC = 100,000,000 zatoshis.

**Dust** — Outputs whose value is below the ZIP-317 marginal fee threshold. Excluded from balances and not eligible as inputs.

---

## Transparent Protocol Extensions

**TEX Transaction** — A Transparent-Exposed Transaction (ZIP-230). A two-step flow that moves shielded funds to a publicly visible transparent address by routing through an ephemeral Refund Address first.

**Refund Address** — An ephemeral transparent address (`TransparentScope::Refund`) generated by the wallet for the first step of a TEX transaction. Not intended for direct use by senders; used internally to bridge the shielded-to-transparent path.

---

## Summary / Display

**TransactionSummary** — A snapshot of a single wallet transaction for display purposes. Not used for internal wallet logic.

**ValueTransfer** — A single directional movement of funds within a transaction: received, sent, shielded, or self-sent.

**TransactionSummaries / ValueTransfers** — Ordered collections of their respective types.

---

## Donation

**Zennies for Zingo** — An opt-in donation feature. The library exposes hardcoded addresses (`ZENNIES_FOR_ZINGO_DONATION_ADDRESS`, per chain type) and a suggested amount (1,000,000 zatoshis = 0.01 ZEC). Callers include this address as a recipient when constructing a send request to donate. Nothing is added automatically.

**Developer Donation Address** — A separate hardcoded address for developer donations, also opt-in.

---

## Privacy

**Tor** — Experimental opt-in privacy layer. `LightClient` can hold a `tor::Client` (from `zcash_client_backend`). Currently only wired to price fetching — not to sync or Transmission. Not suitable for production use.

---

## Persistence

**Wallet File** — The serialized wallet state, stored as `zingo-wallet.dat` inside the wallet directory. zingolib owns all file I/O: consumers provide a directory path and zingolib opens and writes the file itself.

**Save Flow** — Wallet state changes set an internal dirty flag. The `LightClient` save task wakes periodically, detects the dirty flag, serialises the wallet, and atomically writes to disk (temp file → rename, power-safe). Consumers that need an immediate one-shot flush call `LightClient::flush()`.

**Dirty Flag** — An internal boolean on `LightWallet` that records whether unsaved changes exist. Set automatically by all mutating operations; cleared after a successful write. Exposed to external code via `LightWallet::mark_dirty()`.

---

## Consumers

**zingolib** is a Rust library. Its primary integration surface is the `LightClient` API, consumed directly by other Rust crates and programs. One known consumer is **zingo-mobile**, which wraps `zingolib` via a UniFFI-generated FFI layer (Kotlin/Swift). `zingo-cli` is a power-user/developer CLI built on the same library.

---

## Testing

**libtonode-tests** — Integration tests that run zingolib against a real local node stack (library-to-node). Uses `zcash_local_net` to spin up a `Validator` + `Indexer` pair; the pair is selected at compile time (the test-infrastructure glossary's "Network combo"). The no-feature default is `zainod+zebrad`; `lightwalletd+zebrad` survives as an opt-in combo; zcashd-backed combos were removed in July 2026.

**darkside-tests** — Deterministic reorg and edge-case tests that inject arbitrary blocks without mining. Their authoritative home is the mock-indexer darkside module inside zingolib, which runs offline; the standalone `darkside-tests` crate (driving lightwalletd's "darkside" mode) is retired by the package-simplification work, its nine tests having been long ignored.

**Validator** — The consensus node in a test local net: `zebrad`. (zcashd-backed combos were removed from this repo in July 2026.)

**DefaultValidator / DefaultIndexer** — Type aliases in `zingolib_testutils` that resolve to the active backend combo at compile time, allowing test code to remain backend-agnostic.

---

## Utility Crates

**zingo-memo** — Utilities for creating and parsing the Zcash memo field.

**zingo-price** — Price feed integration (Gemini exchange).

**zingo-status** — `ConfirmationStatus` type: the confirmation state of a transaction (calculated, transmitted, mempool, confirmed, etc.).

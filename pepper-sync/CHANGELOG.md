# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added
- `wallet::SyncMode::on_completion`, the pure step a completed scan applies to
  the sync mode: `Running` becomes `Shutdown`, and every other mode is kept.
- `wallet::SyncMode::apply` and `wallet::SyncMode::transition`, which move the
  atomic sync mode in one exchange, by a pure step or from one mode to another,
  and return the mode they replaced. The sync engine and its consumers share
  them in place of hand-written compare-and-swap calls.
- `sync::CHECK_NEW_BLOCKS_INTERVAL`, the interval in seconds at which
  continuous sync checks for newly mined blocks.
- Continuous sync (ADR 0051). BREAKING: `config::SyncConfig` gains a
  `shutdown_on_completion` field. When `false`, `sync` keeps running once the
  wallet reaches the chain tip, checking for newly mined blocks (on mempool
  stream closure and every ten seconds) and scanning them, until the consumer
  sets `SyncMode::Shutdown`. When `true`, sync shuts down and returns once the
  wallet is fully up to date, while still picking up blocks mined during the
  session. Both paths now share a single shutdown sequence.
- `SyncConfig` serialization version bumped to 2 to persist
  `shutdown_on_completion`; version 1 configs read it as `false`.
- Transparent outputs and spends in newly mined blocks are detected from
  compact block transparent data, matched against the wallet's in-use and gap
  addresses, rather than re-running the transparent address RPCs. Historical
  sync still uses the RPCs for address discovery. A gap address found in use
  is moved to the in-use set and the gap is replenished.
  Compact block transparent data is only scanned for blocks mined during the
  sync session (or re-orged during it), as transparent address discovery has
  already located all relevant transactions up to the session's initial chain
  height. This prevents the transparent inputs of all other scanned blocks
  from being stored in the wallet's outpoint map. Transparent address discovery now runs at
  the start of every sync session, even if no blocks were mined since the
  last one.
- BREAKING: `error::SyncError::SyncStatusError` variant, returned when
  publishing sync progress fails.
- BREAKING: `error::SyncStatusError::SyncProgressChannelClosed` variant,
  returned when the sync progress receiver has been dropped.
- BREAKING: `error::ScanError` variants `TransparentOutputInvalidValue`,
  `AllAddressesInUse`, and `TransparentAddressDerivationError`.
- BREAKING: `error::ServerError::TreeStateNotServed` variant, returned when the
  server omits a shielded pool's tree state at or above the pool's activation
  height. The server does not serve that pool, so retrying the same server
  will not succeed: the error recommends
  `SyncRecoveryObservables::ServerUnavailable`, and the consumer should switch
  to a different server and sync again.
- BREAKING: `error::ScanError::TreeSizeNotReported` variant, returned when
  block metadata reports a tree size of zero where the wallet has calculated a
  non-zero tree size.
- BREAKING: `error::ServerError::UnsupportedProtocolVersion` variant, returned
  at the start of sync when the server's `GetLightdInfo`
  `lightwalletProtocolVersion` is missing or below v0.5.0. Such servers do not
  serve the transparent and Ironwood data in compact blocks that sync requires.
  The error recommends `SyncRecoveryObservables::ServerUnavailable`, and the
  consumer should switch to a different server and sync again.
- BREAKING: `error::ServerError::ChainHeightBelowScanRange` variant, returned
  when fetching a scan range fails and the server's chain height is below the
  last block of the scan range. A re-org lowered the chain height after the
  scan range was selected, or the server is behind the chain tip. The fetch
  error was returned before, which recommended
  `SyncRecoveryObservables::ServerUnavailable` where the server answered that
  it had no such block. The new error recommends
  `SyncRecoveryObservables::MaybeRecoverableServer`, and syncing again
  truncates the wallet to the server's chain height and verifies it against
  the server's chain.

### Changed
- Scanning reads version 2 zingo memos, which carry a recipient's unified
  address with its ZIP 316 revision and metadata, through
  `ParsedMemo::into_unified_addresses`.
- BREAKING: the Zcash stack moves to the librustzcash NU7 pre-release cohort:
  `zcash_client_backend` 0.25.0-pre.1, `zcash_keys` 0.17.0-pre.1, `zcash_primitives` 0.31.0-pre.1, `zcash_proofs` 0.31.0-pre.1, `zcash_protocol` 0.11.0-pre.0, `zcash_address` 0.14.0-pre.1, `zcash_transparent` 0.11.0-pre.1, `zcash_encoding` 0.5, `zcash_note_encryption` 0.5, `zcash_script` 0.6, `orchard` 0.16, `sapling-crypto` 0.9, `incrementalmerkletree` 0.9, `shardtree` 0.8, `zip32` 0.3, `bip32` 0.6, `jubjub` 0.11, `secp256k1` 0.33, `rand` 0.10. `LocalNetwork` gains its `nu7` field, the note-size constants
  come from `orchard::note_encryption` and `sapling_crypto::note_encryption`
  where `zcash_note_encryption` exported them, and the `ShieldedOutput` bound
  loses its ciphertext-size parameter.
- A note's recipient full unified address is encoded with every receiver it
  carries, in the wallet file and through
  `OutputInterface::encoded_recipient_full_unified_address`. `zcash_keys`
  0.17 made `UnifiedAddress::encode` omit the transparent receiver of a
  shielded address, which is the sharing form and not the recorded one.
- BREAKING: `wallet::SyncMode::from_atomic_u8` borrows the atomic as
  `&AtomicU8` in place of taking an `Arc<AtomicU8>` by value.
- BREAKING: `client::FetchRequest::CompactBlockRange` has an added `bool`
  field. When true, compact blocks are requested with the `TRANSPARENT`,
  `SAPLING`, `ORCHARD` and `IRONWOOD` pool types, otherwise with the default
  (shielded only). Transparent data is only requested for blocks above the
  transparent scan floor. Previously, no pool types were ever requested, so
  compact blocks never contained the transparent data that is scanned for
  blocks mined during the sync session.
- BREAKING: `wallet::traits::SyncShardTrees::update_shard_trees` takes the
  consensus parameters as its first argument.
- A server that does not serve Ironwood is an error. Failing to fetch Ironwood
  subtree roots fails sync instead of being tolerated, an omitted Sapling,
  Orchard or Ironwood tree state at or above the pool's activation height
  returns `ServerError::TreeStateNotServed`, and a zero tree size in block
  metadata where the wallet has calculated a non-zero tree size returns
  `ScanError::TreeSizeNotReported` instead of being logged as a warning. Both
  errors recommend `SyncRecoveryObservables::ServerUnavailable`.
- `wallet::traits::SyncWallet::get_transparent_addresses` and
  `get_transparent_addresses_mut` document that the returned addresses must be
  in use and must not include gap addresses.
- `sync::SyncResult` `Display` output is headed "Sync result" instead of
  "Sync completed succesfully", since sync may now return on explicit shutdown.
- BREAKING: the truncation rescan announces itself. `error::SyncError::PoolHistoryReopened`
  is now a struct variant carrying the pool, the rescan height, the
  disagreeing block, and both tree sizes, and it renders one
  "RESCAN TRIGGERED" sentence naming the consequence and the cause.
  `error::ScanError::IncorrectTreeSize` gains the disagreeing block's
  `height`, and the truncation site logs at error level where it warned.
- `sync::sync_status`: a session with no scannable blocks or outputs no longer
  reports a finished sync. A stale initial sync state, whose previously scanned
  counts stand at or above the wallet's whole span, saturates the session
  denominator to zero. The status now reports the wallet's total progress for
  that session, where it previously divided zero by zero and coerced the
  resulting `NaN` to one hundred percent. The pool totals and the block span are
  saturated likewise, so a rewound tree bound can no longer underflow them.
- BREAKING: a wrapper error variant renders only its own layer. The
  `Display` texts of the `error::SyncError`, `error::MempoolError`, and
  `error::ScanError` wrapper variants no longer embed the wrapped source's
  text, and `ScanError::EncodingError` renders transparently. A consumer
  recovers the full failure story by walking the `source()` chain.
- `wallet::WalletTransaction::update_status`: added `fail_confirmed` bool for protecting against confirmed txs being
    set to failed in cases other than re-org truncation.
- The readers of `config::SyncConfig`, `config::PerformanceLevel` and the
  `wallet` types (`ScanTarget`, `SyncState`, `TreeBounds`, `NullifierMap`,
  `WalletBlock`, `WalletTransaction`, `TransparentCoin`, `WalletNote`,
  `OutgoingNote`, `ShardTrees`) return an `InvalidData` error for a serialized
  version above the one this build writes. They read on at any version before,
  so an older build read a newer layout as the layout it knew. A consumer's
  wallet file no longer needs a new version of its own for a change to the
  serialized version of one of these types.
- `wallet::SyncState` serialization version bumped to 5 to persist the
  transparent scan floor. Earlier versions read it as unset, and the next sync
  session's transparent address discovery then searches the blocks within the
  re-org allowance of the last known chain height, as before.

### Fixed
- `sync` rolls each shard tree back to the wallet's highest scanned height
  when it starts, where the tree holds a checkpoint above that height. A sync
  session that ended partway through a wallet update left note commitments in
  the trees for blocks that were still to be scanned. When a re-org then
  replaced those blocks, every later session failed with
  `shard tree error ← Inserted root conflicts with existing root` and only a
  rescan from the birthday recovered the wallet. (#2834)
- A `ServerError::RequestFailed` caused by network weather is now recommended
  `SyncRecoveryObservables::MaybeRecoverableServer` and
  `recommend_same_server`, rather than `ServerUnavailable`. Network weather is
  a failure raised by the transport (a tonic transport error, client-side
  timeout or I/O error in the status's source chain) or a status code gRPC
  names as transient (`Unavailable`, `DeadlineExceeded`, `Cancelled`,
  `ResourceExhausted`, `Aborted`). Any other status code is an answer from a
  server that cannot serve the request and is still recommended
  `ServerUnavailable`. `MaybeRecoverableServer` now documents that callers
  should retry a bounded number of times before treating the server as
  unavailable. (#2799)
- The mempool monitor counts a mempool transaction as unprocessed before
  sending it to the sync engine rather than after. The count was incremented
  only once the send completed, so the mempool drain could observe a zero
  count while a transaction was queued and end the sync session without
  processing it, leaving the transaction's wallet record in `Transmitted`
  status until the next session.
- Subtree roots are fetched, and the initial frontier added, at the start of
  every sync session before scanning begins, even if no blocks were mined
  since the last session (#2782). Continuous sync had moved this behind a new
  block check, so a session restarted after a pool rescan could insert note
  commitments before fetching the pool's subtree roots. The shard store then
  hides the lower shards' missing roots from subtree root fetching, leaving
  the wallet unable to compute the pool's tree root.
- A pool rescan (`SyncError::PoolHistoryReopened`) clears the rescanned pool's
  shard ranges along with its shard tree, so they are rebuilt from the subtree
  roots fetched in the next session.
- A pool rescan clears the shard trees of the rescanned pool and any pool
  activated after it, and truncates the pools activated before it, as
  documented. The condition was inverted, so an Ironwood rescan cleared the
  Sapling and Orchard shard trees, losing their note commitments below the
  Ironwood activation height.
- A `ScannedWithoutMapping` range is only selected to re-fetch its nullifiers
  once it is the first unscanned range. It could also be selected as the
  highest priority range while a lower range was still scanning, so the
  re-fetched nullifiers were discarded and fetched again.
- Transparent transactions in blocks mined during a sync session that ended
  before scanning them are found by the next session. Blocks mined during a
  session are above its transparent scan floor and are only covered by
  scanning their compact block transparent data. The next session set its
  floor above them, and its transparent address discovery only searched the
  blocks within the re-org allowance of the last known chain height, so
  transactions in older unscanned blocks were never found. The transparent
  scan floor is now stored in the wallet's sync state, and transparent address
  discovery searches from the floor of the previous session.
- Transparent transactions are found again when the server reports a chain
  height below the wallet's during a sync session and the chain then extends.
  The wallet was truncated to the chain height without lowering the
  transparent scan floor, so the blocks scanned in place of the truncated
  blocks at or below the floor had neither their compact block transparent
  data scanned nor transparent address discovery performed. Transparent
  transactions mined in them were left in `Failed` status and transparent
  spends were left undetected.
- Transparent funds received by a gap address in a block mined during the sync
  session are detected after nullifiers have been re-fetched. Every scan
  returned the full set of gap addresses, which replaced the scanner's, and
  re-fetching the nullifiers of a `ScannedWithoutMapping` range returned an
  empty set. Compact block transparent data was then scanned with no gap
  addresses for the rest of the session. Scans now return only the gap
  addresses found in use and the gap addresses derived to replace them, and
  these changes are applied to the scanner's gap addresses.
- A transaction mined during the sync session that spends the wallet's
  transparent coins and pays everything to external recipients is fetched and
  confirmed when the spend is detected (#2798). Compact block scanning mapped
  its transparent inputs and marked the coins spent, but only a transaction
  with an output to the wallet was targeted for a full scan. The spending
  transaction was left in `Mempool` status until it passed its expiry height
  and was marked failed, which also reset the spent coins to unspent.
- The mempool monitor stops when `sync` returns an error or its future is
  dropped (#2828). Only a clean shutdown told the monitor to stop, so after
  any other exit it held its `GetMempoolStream` open until the next block or
  mempool transaction, and retried a refused stream request every three
  seconds for the life of the process. A clean shutdown also waited on those
  retries, so `sync` hung while the server refused the stream.
- Sync with `shutdown_on_completion` set keeps a pause set by the consumer. On
  completion the sync mode was set to `SyncMode::Shutdown` whatever it held, so
  a `SyncMode::Paused` set by the consumer since the sync mode was last read
  was replaced, and sync ran its shutdown sequence while the consumer held it
  paused. Completion now applies `SyncMode::on_completion` in one atomic
  exchange, which sets `Shutdown` over `Running` alone, and a paused sync
  shuts down once the consumer resumes it and it completes again.
- Scan results of a scan range that a re-org truncated or re-prioritised while
  it was being scanned are discarded, and the part of the range the wallet
  still holds is scanned again. When the server reported a chain height below
  the wallet's during a sync session with a scan of the chain tip range in
  flight, the wallet truncated the range and then panicked while processing
  the scan results. An error returned by such a scan is discarded in the same
  way, where it ended the sync session before.
- A scan task with `Verify` priority is scanned as one load. When its
  continuity check fails, re-org handling resets the scan range of the failed
  scan, which panicked when the loader had split the range into several
  loads.
- A scan range whose first block does not follow the block below it is
  verified again within the sync session, whatever priority it was selected
  with. Only a scan range selected with `Verify` priority was handled before,
  and any other ended the sync session with a continuity error. The block
  below is held by the wallet, or was kept by the loader from an earlier scan,
  and a re-org has replaced it since. The scan range is set back to the
  priority it was selected with, its first blocks are set to `Verify` and the
  scanner returns to verifying, so the continuity check failing again is
  handled as a re-org.
- A re-org reopens the scanned ranges above the verification range to be
  scanned again. Truncating the wallet removes the wallet data of every block
  above the truncation height, and a scanned range above the verification
  range kept its `Scanned` priority with its wallet data removed.

### Removed

## [0.5.0] - 2026-06-10

### Added
- `error::SyncRecoveryObservables` enum with variants `MaybeRecoverableServer`,
  `ServerUnavailable`, and `Abort` — classifies sync errors for consumer retry logic.
- `error::SyncError::is_retryable()` — returns `true` for transient errors
  (server timeouts, connection drops, mempool failures).
- `error::SyncError::recovery_recommendation()` — maps any sync error to a
  `SyncRecoveryObservables` without callers needing to match on error internals.
- `error::ServerError::is_retryable()` — distinguishes transport failures from
  invalid server data.
- `error::ServerError::recovery_recommendation()` — server-level recovery classification.

### Changed
- `wallet::TransparentCoin`: serialized version incremented to 1 to serialize output indexes as u32
- `wallet::WalletNote`: serialized version incremented to 2 to serialize output indexes as u32
- `wallet::OutgoingNote`: serialized version incremented to 1 to serialize output indexes as u32
- `wallet::OutputId`:
  - `output_index` field is now u32.
  - `new` constructor `output_index` parameter is now u32.
  - `output_index` method's return type is now u32.

## [0.4.0] - 2026-06-05

### Added
`wallet::WalletTransaction`: added `total_external_outgoing_note_value` method

## [0.3.0]

### Changed

- `sync::sync` fn: `client` parameter now takes a `CompactTxStreamerClient<tonic::Channel>`

## [0.2.0] - 2026-02-26

### Added
- `wallet::WalletTransaction::update_status`
- `wallet::WalletTransaction::new_for_test`
- `sync::set_transactions_failed` - also re-exported in lib.rs

### Changed
- `error::SyncError`:
  - added `BirthdayBelowSapling` variant which is returned when `sync` is called with wallet birthday below sapling activation height.
  - `ChainError` variant now includes the wallet height and chain height.
- `error::ScanError`:
  - `InvalidMemoBytes` variant now uses `zcash_protocol::memo::Error` instead of deprecated `zcash_primitives::memo::Error` type.
- `keys::KeyID` now uses `zip32::AccountId` directly instead of `zcash_primitives` re-export.
- `keys::ScanningKeyOps` trait now uses `zip32::AccountId` directly instead of `zcash_primitives` re-export.
- `keys::TransparentAddressId` now uses `zip32::AccountId` directly instead of `zcash_primitives` re-export.
- `sync::ScanPriority`:
  - added `RefetchingNullifiers` variant.
- `wallet::SyncState`:
  - incremented to serialized version 3 to account for changes to `ScanPriority`
  - `wallet_height` method renamed to `last_known_chain_height`.
- `wallet::NoteInterface` trait: added `refetch_nullifier_ranges` method.
- `wallet::SaplingNote`:
  - implemented `refetch_nullifier_ranges` method.
  - updated serialization to account for new `WalletNote` field.
- `wallet::OrchardNote`:
  - implemented `refetch_nullifier_ranges` method.
  - updated serialization to account for new `WalletNote` field.
- `wallet::WalletNote`:
  - incremented to serialized version 1 to account for changes to `WalletNote` struct.

### Removed

## [0.1.0] - 2026-01-09

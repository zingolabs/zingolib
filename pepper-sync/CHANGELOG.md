# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Deprecated

### Added
- `sync::SyncStatus::total_blocks`, the block span from the wallet birthday to
  the last known chain height, the denominator `sync_status` already divided
  by and never reported.
- `sync::SyncStatus::nullifier_retrieval_pending`, whether any scan range still
  awaits nullifier mapping or refetching.
- `sync::SyncResult::total_outputs_scanned`, `sync::SyncResult::total_outputs`,
  and `sync::SyncResult::nullifier_retrieval_pending`, copied from the final
  sync status of the session, so a consumer can derive the session's total
  progress from the result alone.
- `sync::ScanRange::encloses` and `sync::ScanRange::overlaps`, the geometry of
  a scan range against a block range, and `sync::ScanPriority::in_flight`, the
  priority the wallet holds a selected range at while its scan task runs.
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
- BREAKING: `error::ScanError::TransactionNotServed` variant, returned when
  the server does not serve a transaction that its own data placed in the
  chain: one the sync engine discovered through trial decryption, spend
  detection or transparent address discovery. The served data is inconsistent,
  so the error recommends `SyncRecoveryObservables::ServerUnavailable`, and
  the consumer should switch to a different server and sync again. A
  transaction a consumer's scan target names is not covered: that target is
  dropped instead.
- `wallet::ScanTarget::new`, the one constructor of a consumer's scan target
  from a block height and an optional txid, and the `block_height` and `txid`
  accessors.

### Changed
- Scanning places a shard tree checkpoint only on the blocks the wallet
  will hold one for: the top `MAX_SHARDTREE_CHECKPOINTS` blocks of a range
  scanned above the highest scanned height, the blocks of a lower range
  inside that window, and the anchor retention boundaries inside their
  window. Every other block's checkpoint was built, inserted and pruned
  from each pool's tree for nothing, a fixed cost per block per active
  pool that grew with the ironwood pool. The heights come from one function,
  `witness::checkpoint_heights`, which the shard tree update also uses to
  decide the checkpoints it adds for blocks that leave a pool's tree
  unchanged, so the two sides agree. A scan task carries the wallet's
  highest scanned height from its selection for this.
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
- `sync::sync_status`: the session counts saturate against a stale initial
  sync state, whose previously scanned counts stand at or above the wallet's
  whole span, instead of underflowing. The pool totals and the block span are
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
- The ironwood subtree roots are fetched on every chain. The wallet checks the
  lightwallet protocol version to ensure ironwood is served, so a server that
  does not serve them is an error rather than a chain without NU6.3.
- BREAKING: `wallet::ScanTarget` has private fields. A consumer constructs a
  target through `ScanTarget::new(block_height, Option<TxId>)` and reads it
  through its accessors; the `narrow_scan_area` flag and the all-zero txid
  standing for "no transaction" are gone. Two targets naming the same block
  height and txid are the same target, and adding one merges it into the one
  the sync state holds. A target also records, internally, the shielded pools
  its transaction was found to hold notes of and whether the sync engine or
  the consumer added it. `ScanTarget` is serialized as version 1, carrying the
  optional txid, the pools and the origin; a version 0 target is read as a
  consumer's target with no pool, a zero txid becoming none. (#2862)
- The narrow range of blocks prioritised around a scan target is 1,000 blocks,
  down from 10,000. A detected spend prioritises the narrow range around the
  spending transaction rather than the whole shard of the spent note's pool: a
  spent note needs no witness, and a change note in the spending transaction
  prioritises its own pool's shard when it is found.
- A pool rescan targets the wallet's transactions above the rescan height as
  the sync engine's own rather than as a consumer's, since the wallet's
  records of the pool are wrong and the chain's are not, so one the server no
  longer serves is reported as an inconsistency.

### Fixed
- `sync` makes the server requests of its session start, the chain height,
  the tree states that bound the session and the subtree roots, with the
  wallet lock released, and takes the lock only to apply what they return. A
  consumer reading the wallet, including `sync_status` and a save, no longer
  waits for a slow server to answer them. Against zaino, whose
  `GetSubtreeRoots` serves one block per subtree, a fresh wallet held the lock
  for the whole fetch. (#2961)
- `sync` observes a `SyncMode::Shutdown` request while a session-start server
  request is pending, abandons the request and returns its result, instead of
  shutting down only once the request is answered or fails. The abandoned
  request is left to the fetcher, which is aborted at shutdown rather than
  joined. (#2962)
- A transaction request that zaino refuses with "retry with a fresh snapshot",
  because it fell between two mempool snapshots, is sent again, at most three
  times, instead of ending the sync session with a server error.
- `sync` rolls each shard tree back to the wallet's highest scanned height
  when it starts, where the tree holds a checkpoint above that height. A sync
  session that ended partway through a wallet update left note commitments in
  the trees for blocks that were still to be scanned. When a re-org then
  replaced those blocks, every later session failed with
  `shard tree error ← Inserted root conflicts with existing root` and only a
  rescan from the birthday recovered the wallet. (#2834)
- `sync` makes every server request that the processing of scan results
  depends on before it updates the wallet, so a failed request leaves the
  wallet as it was. Spend detection only reads the wallet's nullifier and
  outpoint maps, and a spend stays mapped until the cleanup drops it behind the
  fully scanned height, so recording a spend on its note or coin can be
  repeated. A failed request for a spending transaction used to remove the
  spend from the map with the note left unspent, and a later sync session
  could miss the spend. A pool rescan fetches its frontier before it clears
  the pool's records, where a failed request used to remove the pool's
  transactions with their scan ranges still recorded as scanned. (#2834)
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
- Scan results of a load whose scan range a re-org truncated or re-prioritised
  while it was being scanned, or over which a later scan task was selected, are
  discarded, and the blocks of the load the wallet still holds for that task
  are scanned again. The decision is per task: a load names the task it belongs
  to, and the engine keeps the tasks in flight in selection order. When the
  server reported a chain height below the wallet's during a sync session with
  a scan of the chain tip range in flight, the wallet truncated the range and
  then panicked while processing the scan results. An error returned by such a
  scan is discarded in the same way, where it ended the sync session before.
- Re-org handling resets the blocks of the failed `Verify` load within
  whichever wallet range holds them, so the loader splits a `Verify` scan task
  at a load budget like any other. Before, the reset panicked when the loader
  had split the range, and a `Verify` task scanned as one load bypassed the
  output and nullifier budgets.
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
- `sync` fetches subtree roots from the newest recorded shard range rather than
  the shard tree's stored shard count. Scanning advances that count past the
  roots the server holds, so after the first sync session the fetch returned
  nothing and the shard range list never grew: every shard the wallet completed
  by scanning had no shard range, and the trailing "incomplete shard" the
  spendability check and the chain tip priority range start from reached back
  to a root that could be months old. The ranges are now rebuilt from the fetch
  index, so range `i` belongs to subtree root `i`, and the newest recorded root
  is fetched again every session, which corrects a range whose completing block
  a re-org moved. An existing wallet catches up on its next session. (#2951)
- A fetched subtree root completing at or below the recorded newest height adds
  no shard range. After a shard tree rollback refetched its bare roots, the
  root completing at the recorded height pushed a degenerate one-block range.
  Shard ranges are contiguous: each spans at least two blocks, each starts at
  the completing block of the one before, and their ends strictly increase.
  (#2952)
- A fetched subtree root for a shard that scanning populated but left with a
  gap becomes the cached annotation of the shard's root node, so the shard's
  root is computable before the gap is scanned and the leaves scanned in
  afterwards are checked against it. The root used to be skipped, leaving a
  shard that completed on chain while partially scanned without a computable
  root, and a witness that needed it failed although the note's shard and the
  anchor's shard were scanned. (#2954)
- The scan ranges prioritised around a scan target at the start of a sync
  session follow the pools the target's transaction holds notes of, recorded
  when the notes are found: the whole shard of each such pool, and the narrow
  range of blocks around a target with none. Every target used to be
  prioritised as an ironwood target, which `determine_block_range` resolved to
  the orchard shard before NU6.3 and the ironwood shard after, so a sapling
  note's shard, or an orchard note's shard once ironwood had completed shards,
  could be left at `Historic` behind the whole unscanned history. (#2957)
- A scan target that recorded a pool is kept until, for every such pool, its
  block lies below the completing block of the pool's newest recorded shard
  and every recorded shard range holding the block is scanned through its
  completing block. The target used to be dropped as soon as the fully scanned
  height passed it, so the rest of a shard mined while the wallet was closed
  arrived as `Historic` with nothing left to prioritise it. A target on the
  newest completing block is kept, as that block also holds the first
  commitments of the incomplete shard. (#2958)
- A consumer's scan target whose transaction the server does not serve is
  dropped with a warning and the scan continues, where it used to fail every
  scan of its range. A consumer's target whose block height lies below the
  wallet birthday or above the chain height is dropped at the start of the
  session. Network weather on any target is reported as the transient failure
  it is and drops nothing. (#2959)
- A re-org truncation removes the scan targets above the truncation height
  with the wallet's blocks, transactions, nullifiers and outpoints. A target
  above it names a transaction in a block the wallet no longer holds and asked
  the server for it when the replacing blocks were scanned. (#2960)

### Removed
- **Breaking.** `sync::SyncStatus::percentage_total_blocks_scanned`,
  `sync::SyncStatus::percentage_total_outputs_scanned`, and
  `sync::SyncResult::percentage_total_outputs_scanned`. The engine reports the
  counts and spans the percentages were derived from; the derivation, with its
  clamp, its reading of an empty span as complete, and its one-step-short
  reading while nullifiers await retrieval, now lives in
  `zingolib::perspective::sync_progress`.
- **Breaking.** The `Display` and `From<_> for json::JsonValue` implementations
  of `sync::SyncStatus` and `sync::SyncResult`. `zingolib::perspective::sync_progress`
  renders both, with the same text and the same JSON keys. pepper-sync no longer
  depends on the `json` crate.
- **Breaking.** `sync::SyncStatus::percentage_session_blocks_scanned` and
  `sync::SyncStatus::percentage_session_outputs_scanned`. These were the only fields whose
  computation could abort the process, and no consumer read them: the CLI's progress
  surfaces take `total_outputs_scanned`, `total_outputs`, and `is_complete()`, and
  `SyncResult` carries the total percentage only. A consumer that wants a session
  percentage computes it from the session and total counts the struct still reports. The
  two fields also leave the `json::JsonValue` rendering of a sync status.

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

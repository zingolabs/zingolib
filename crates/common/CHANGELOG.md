# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- The crate holds the types a zingo wallet file stores and a consumer needs
  without the wallet, split out of `zingolib`. `zingolib` re-exports each at
  its old path.
- `chain` module: `ChainType` and `InvalidChainType`.
- `keys` module: `UnifiedKeyStore`, `UnifiedAddressId`, `ReceiverSelection`,
  `KeyError`, `encode_ufvk` and the `KEY_TYPE_EMPTY`, `KEY_TYPE_VIEW` and
  `KEY_TYPE_SPEND` constants. `UnifiedKeyStore::determine_nth_valid_sapling_diversifier`
  is public.
- `serialization` module: the `ReadableWriteable` trait, its implementations
  for the legacy orchard and sapling key types, and `read_version`, which
  reads a serialized version byte and refuses one above the given version.
- `status` module: `ConfirmationStatus` and its codec, which were the
  `confirmation_status` module of `zingo-status` 0.3.0. That crate is
  retired.
- `memo` module: the encoding of the wallet's addresses in a memo field,
  `ParsedMemo`, `PackedMemo`, `MEMO_LEN`, `create_wallet_internal_memo`, the
  deprecated `create_wallet_internal_memo_version_1`, `parse_zingo_memo`
  and the readers and writers of the raw unified address encodings, which
  were the whole of `zingo-memo` 0.3.0. That crate is retired. The memo bytes are
  unchanged.
- `price` module: `Price` and `PriceList` with the codec of the price list,
  which were part of `zingo-price` 0.3.0. The price fetch is the
  `price` module of `zingolib`, built with its `nym` feature.
- The wallet data that the sync engine of `pepper-sync` writes, split out of
  `pepper-sync`, which re-exports each item at its old path. The `wallet`
  module holds `ScanTarget`, `TreeBounds`, `OutputId`, `NullifierMap`,
  `WalletBlock`, `WalletTransaction`, `WalletNote`, `OutgoingNote`,
  `TransparentCoin`, `ShardTrees`, the note and shard store aliases, the
  output traits, `PoolActivation`, `pool_upgrade`, `MAX_REORG_ALLOWANCE` and
  `SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE`. The `keys` module gains `KeyId`,
  `AddressIndex`, `decode_address`, `decode_unified_address` and the
  `keys::transparent` module with `TransparentAddressId`, `TransparentScope`
  and `encode_address`. The `shardtree_ext` module holds the domain-typed
  wrappers over the boolean operations of shardtree.
- `sync` module: the progress record of the sync engine of `pepper-sync`,
  split out of `pepper-sync`, which re-exports each item at its old path. It
  holds `SyncState` with its scan-range logic, `InitialSyncState`,
  `ScanRange`, `ScanPriority` and `add_scan_targets`, and re-exports
  `MAX_REORG_ALLOWANCE` and `SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE`.
- `config` module: `SyncConfig`, `PerformanceLevel`,
  `TransparentAddressDiscovery` and `TransparentAddressDiscoveryScopes`, the
  `config` module of `pepper-sync` 0.7.0, which re-exports it at its old
  path.
- `engine` module, which holds the operations that the sync engine of
  `pepper-sync` runs on the wallet data and the progress record, and which
  `pepper-sync` re-exports in `pepper_sync::engine`. It holds one sealed
  extension trait per type, which no other crate can implement:
  `SyncStateOps`, `InitialSyncStateOps`, `ScanTargetOps`, `WalletBlockOps`,
  `WalletTransactionOps`, `TransparentCoinOps`, `WalletNoteOps`,
  `OutgoingNoteOps`, `KeyIdOps` and `ShardTreesOps`. It also holds
  `ScanTargetOrigin`, `VerifyEnd`, `empty_shard_tree`, `overlaps`,
  `encode_orchard_receiver`, `SHARD_HEIGHT` and `VERIFY_BLOCK_RANGE_SIZE`.
  The other modules expose the paths and the inherent methods that
  `pepper-sync` 0.7.0 exposed.
- `serialization` feature, on by default in the crate and enabled by the
  `wallet_essentials` feature of `pepper-sync`. It adds the
  `wallet::serialization` module with the `read` and `write` codecs of the
  wallet data and `write_string`, the codecs of `SyncState` and of the
  `config` types, and the `jubjub` and `zcash_client_backend` dependencies.
  The codecs of the key store are in every build.
- `test-features` feature, which exposes the test constructors of the wallet
  types and of `SyncState`, the `*_for_test` methods and
  `shardtree_ext::assert_raw_boolean_calls_confined`, which fails when the
  sources of a crate call a shardtree boolean operation outside the wrapper.

### Changed

### Deprecated

### Removed

### Fixed

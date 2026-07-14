# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Removed

### Deprecated

## [5.0.1] - 2026-06-08

### Changed

- `Indexer` and `TransparentIndexer` traits:
  - methods now return `tonic::Status` error to be more compatible as drop-in replacement for ComapctTxStreamerClient
  - methods take a `&mut self` to allow for re-use of client instead of forcing creation of client for each rpc call
  - method returns constrained to impl `Send`
- `GprcIndexer` struct:
  - implementations updated for changes to `Indexer` and `TransparentIndexer` traits
  - `new` constructor is now async
  - `get_client` method renamed `get_clear_net_client`. naming chosen to distinguish against nym enabled clients which will also be held by `Grpcndexer`


### Removed

- `GprcIndexer::get_zcb_client`: crate now unified to use `lightwallet_protocol` types throughout

## [4.0.0]

### Added

- `Indexer` trait covering the full `CompactTxStreamer` gRPC service:
  `get_info`, `get_latest_block`, `send_transaction`, `get_tree_state`,
  `get_block`, `get_block_range`, `get_transaction`, `get_mempool_tx`,
  `get_mempool_stream`, `get_latest_tree_state`, `get_subtree_roots`.
- `TransparentIndexer: Indexer` sub-trait in `src/globally_public.rs`
  for transparent address methods: `get_taddress_txids` (deprecated),
  `get_taddress_transactions`, `get_taddress_balance`,
  `get_taddress_balance_stream` (client-streaming via `Vec<Address>`),
  `get_address_utxos`, `get_address_utxos_stream`.
- `Indexer::ping()` method for server latency testing.
- Per-method error enums for every trait method (`src/error.rs`), each
  with `GetClientError` (connection) and a method-specific `tonic::Status`
  variant. `SendTransactionError` adds `SendRejected`. All bounded by
  `std::error::Error`.
- `TransparentIndexer` per-method error enums in `error::transparent`
  submodule (gated by `globally-public-transparent`).
- Unit test suite for every error enum variant (`error::tests`,
  `error::transparent::tests`).
- Doc-test on every error enum proving the contract (`From` conversions,
  variant matching). Feature-gated doc-tests use `#[cfg]` so
  `cargo test --doc` passes with or without features.
- `GrpcIndexer` struct implementing `Indexer` (and `TransparentIndexer`)
  over gRPC. Validates URI at construction (`new` returns `Result`) and
  pre-builds the TLS endpoint.
- `get_client` inherent method on `GrpcIndexer` returning
  `CompactTxStreamerClient<Channel>` from `lightwallet_protocol`.
- `pub use lightwallet_protocol` re-export so consumers can access proto
  types via `zingo_netutils::lightwallet_protocol::*`.
- Feature gates (all off by default):
  - `globally-public-transparent` — `TransparentIndexer` sub-trait and
    `GrpcIndexer` implementation. Pulls in `tokio-stream`.
  - `ping-very-insecure` — `Indexer::ping()`. Name mirrors the
    lightwalletd `--ping-very-insecure` CLI flag required server-side.
  - `back_compatible` — `GrpcIndexer::get_zcb_client()` returning
    `zcash_client_backend`'s `CompactTxStreamerClient<Channel>` for
    pepper-sync compatibility.
- Deprecated trait methods: `get_block_nullifiers`,
  `get_block_range_nullifiers`, `get_taddress_txids`.
- Compile-time proto agreement tests (`src/proto_agreement.rs`): 20
  dead-code async functions that reference both the generated client
  method and the trait method with explicit type annotations. If either
  side drifts, compilation fails.
- Integration test `get_block_range_supports_descending_order` verifying
  descending block range ordering against a public indexer.

### Changed

- **Breaking:** Replace `zcash_client_backend` with `lightwallet-protocol`
  for all proto-generated types. Consumers must update imports.
- **Breaking:** `GrpcIndexer::new(uri)` now returns `Result<Self, GetClientError>`
  (validates scheme and authority at construction).
- **Breaking:** `uri()` returns `&http::Uri` (not `Option`).
- **Breaking:** Per-method error types (`GetInfoError`, `GetLatestBlockError`,
  `SendTransactionError`, `GetTreeStateError`) replace the single
  `GrpcIndexerError`.
- **Breaking:** Renamed `get_trees` to `get_tree_state`; now takes
  `BlockId` instead of `u64`, matching the proto (`GetTreeState(BlockID)`).
- **Breaking:** Renamed `GetTreesError` to `GetTreeStateError`.
- `get_block_range` documents both ascending (`start <= end`) and
  descending (`start > end`) ordering per the proto spec.
- Bump `tonic` to `0.14`, `lightwallet-protocol` to `0.3`.
- `hyper`, `hyper-rustls`, `hyper-util` moved from dependencies to
  dev-dependencies.
- `cargo doc` requires `--all-features` for intra-doc links to
  feature-gated items (`TransparentIndexer`, `Indexer::ping`,
  `GrpcIndexer::get_zcb_client`).

### Removed

- `zcash_client_backend` dependency (available optionally via
  `back_compatible`).
- `set_uri`, `disconnect`, `disconnected` methods.
- `GrpcIndexerError` unified error type.
- `GetClientError::NoUri` variant.
- `Option<http::Uri>` internal state — `GrpcIndexer` always holds a valid URI.
- `client` module, `GrpcConnector`, `UnderlyingService`, free `get_client`
  function.
- Direct dependencies on `tower`, `webpki-roots`, `zebra-chain`.

## [1.1.0]

NOT PUBLISHED

# zingo-netutils

*Test counts reflect the `dev` branch and update on each push.*

![unit tests](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/zancas/56329b760b39044631c42c0b36dcb2e5/raw/unit.json)
![doc tests](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/zancas/56329b760b39044631c42c0b36dcb2e5/raw/doc.json)
![integration tests](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/zancas/56329b760b39044631c42c0b36dcb2e5/raw/integration.json)

A complete `Indexer` abstraction for communicating with Zcash chain indexers
(`zainod`).

## Organizing principle

This crate defines the **`Indexer` trait** -- the sole interface a Zcash
wallet or tool needs to query, sync, and broadcast against a chain indexer.
The trait is implementation-agnostic: production code uses the provided
`GrpcIndexer` (gRPC over tonic), while tests can supply a mock implementor
with no network dependency.

All proto types are provided by
[`lightwallet-protocol`](https://crates.io/crates/lightwallet-protocol)
and re-exported via `pub use lightwallet_protocol` so consumers do not need
an additional dependency.

## Usage

```rust
use zingo_netutils::{GrpcIndexer, Indexer};

let uri: http::Uri = "https://zec.rocks:443".parse().unwrap();
let indexer = GrpcIndexer::new(uri)?;

let info = indexer.get_info().await?;
let tip  = indexer.get_latest_block().await?;
```

`GrpcIndexer::new` validates the URI (scheme must be `http` or `https`,
authority must be present) and pre-builds the TLS endpoint. It returns
`Result<Self, GetClientError>` -- there is no invalid-state representation.

## Backwards compatibility

Code that needs a raw `CompactTxStreamerClient<Channel>` (e.g. pepper-sync)
can still obtain one:

```rust
let client = indexer.get_client().await?;
```

## Feature gates

All features are **off by default**.

| Feature | What it enables |
|---|---|
| `globally-public-transparent` | `TransparentIndexer` sub-trait: t-address balance, transaction history, and UTXO queries. Requires `tokio-stream`. |
| `ping-very-insecure` | `Indexer::ping()` method. Name mirrors the server-side `--ping-very-insecure` CLI flag. Testing only. |

## Error handling

Every trait method has a dedicated error enum (defined in `src/error.rs`).
Each enum has two variants:

- `GetClientError(GetClientError)` — the connection was never established.
  `InvalidScheme` and `InvalidAuthority` are deterministic; `Transport`
  may be transient and retryable.
- A method-specific gRPC variant wrapping `tonic::Status` — the server
  received the request but returned an error.

`SendTransactionError` adds a third variant, `SendRejected(String)`, for
transactions the server evaluated and rejected (not retryable with the
same bytes).

All error types are bounded by `std::error::Error`. Transparent method
errors live in `error::transparent` (gated by `globally-public-transparent`).

Every error enum has a doc-test proving the contract: `From<GetClientError>`
produces the connection variant, `From<tonic::Status>` produces the
method-specific variant. Feature-gated doc-tests use `#[cfg]` so
`cargo test --doc` passes with or without features, and assertions
execute when the feature is enabled.

| Error type | Used by |
|---|---|
| `GetClientError` | `GrpcIndexer::new`, `get_client`, embedded in every method error |
| `GetInfoError` | `get_info` |
| `GetLatestBlockError` | `get_latest_block` |
| `SendTransactionError` | `send_transaction` |
| `GetTreeStateError` | `get_tree_state` |
| `GetBlockError` | `get_block` |
| `GetBlockNullifiersError` | `get_block_nullifiers` |
| `GetBlockRangeError` | `get_block_range` |
| `GetBlockRangeNullifiersError` | `get_block_range_nullifiers` |
| `GetTransactionError` | `get_transaction` |
| `GetMempoolTxError` | `get_mempool_tx` |
| `GetMempoolStreamError` | `get_mempool_stream` |
| `GetLatestTreeStateError` | `get_latest_tree_state` |
| `GetSubtreeRootsError` | `get_subtree_roots` |
| `PingError` | `ping` |
| `GetTaddressTxidsError` | `get_taddress_txids` |
| `GetTaddressTransactionsError` | `get_taddress_transactions` |
| `GetTaddressBalanceError` | `get_taddress_balance` |
| `GetTaddressBalanceStreamError` | `get_taddress_balance_stream` |
| `GetAddressUtxosError` | `get_address_utxos` |
| `GetAddressUtxosStreamError` | `get_address_utxos_stream` |

## Building docs

Docs must be built with `--all-features` so intra-doc links to
feature-gated items resolve:

```sh
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --document-private-items
```

## TLS

HTTPS connections use rustls with webpki root certificates (via tonic's
`tls-webpki-roots` feature). No system OpenSSL installation is required.

## gRPC endpoint audit

Every `CompactTxStreamer` RPC from `walletrpc/service.proto` is covered.
The table below shows the mapping between proto RPCs and trait methods.

### Exact matches (13)

| Proto RPC | Trait method | Input | Output |
|---|---|---|---|
| `GetTreeState(BlockID)` | `get_tree_state(BlockId)` | `BlockId` | `TreeState` |
| `GetBlock(BlockID)` | `get_block(BlockId)` | `BlockId` | `CompactBlock` |
| `GetBlockNullifiers(BlockID)` | `get_block_nullifiers(BlockId)` | `BlockId` | `CompactBlock` |
| `GetBlockRange(BlockRange)` | `get_block_range(BlockRange)` | `BlockRange` | `Streaming<CompactBlock>` |
| `GetBlockRangeNullifiers(BlockRange)` | `get_block_range_nullifiers(BlockRange)` | `BlockRange` | `Streaming<CompactBlock>` |
| `GetTransaction(TxFilter)` | `get_transaction(TxFilter)` | `TxFilter` | `RawTransaction` |
| `GetTaddressTxids(TABF)` | `get_taddress_txids(TABF)` | `TABF` | `Streaming<RawTransaction>` |
| `GetTaddressTransactions(TABF)` | `get_taddress_transactions(TABF)` | `TABF` | `Streaming<RawTransaction>` |
| `GetTaddressBalance(AddressList)` | `get_taddress_balance(AddressList)` | `AddressList` | `Balance` |
| `GetMempoolTx(GetMempoolTxRequest)` | `get_mempool_tx(GetMempoolTxRequest)` | `GetMempoolTxRequest` | `Streaming<CompactTx>` |
| `GetSubtreeRoots(GetSubtreeRootsArg)` | `get_subtree_roots(GetSubtreeRootsArg)` | `GetSubtreeRootsArg` | `Streaming<SubtreeRoot>` |
| `GetAddressUtxos(GetAddressUtxosArg)` | `get_address_utxos(GetAddressUtxosArg)` | `GetAddressUtxosArg` | `GetAddressUtxosReplyList` |
| `GetAddressUtxosStream(GetAddressUtxosArg)` | `get_address_utxos_stream(GetAddressUtxosArg)` | `GetAddressUtxosArg` | `Streaming<GetAddressUtxosReply>` |
| `Ping(Duration)` | `ping(ProtoDuration)` | `ProtoDuration` | `PingResponse` |

### Intentional divergences (7)

| Proto RPC | Trait method | What diverges | Why |
|---|---|---|---|
| `GetLightdInfo(Empty)` | `get_info()` | Hides `Empty` arg | Ergonomic -- unit request type adds no information |
| `GetLatestBlock(ChainSpec)` | `get_latest_block()` | Hides `ChainSpec` arg | Same reason |
| `GetMempoolStream(Empty)` | `get_mempool_stream()` | Hides `Empty` arg | Same reason |
| `GetLatestTreeState(Empty)` | `get_latest_tree_state()` | Hides `Empty` arg | Same reason |
| `SendTransaction(RawTransaction)` | `send_transaction(Box<[u8]>)` | Abstracts input and output | Takes raw bytes, returns txid string -- decouples callers from proto |
| `GetTaddressBalanceStream(stream Address)` | `get_taddress_balance_stream(Vec<Address>)` | `Vec` instead of client stream | Impl converts to stream internally; simpler call-site API |
| `Ping(Duration)` | `ping(ProtoDuration)` | Type alias | Avoids collision with `std::time::Duration` |

### Compile-time enforcement

The `proto_agreement` test module (`src/proto_agreement.rs`) contains 20
dead-code async functions that reference both the generated
`CompactTxStreamerClient` method and the corresponding trait method with
explicit type annotations. If either side drifts from the proto, the
module fails to compile.

### Full RPC / method / error audit

Every proto RPC has a 1:1 trait method and a 1:1 error enum.

| # | Proto RPC | Trait method | Error enum |
|---|---|---|---|
| 1 | `GetLightdInfo(Empty)` | `get_info` | `GetInfoError` |
| 2 | `GetLatestBlock(ChainSpec)` | `get_latest_block` | `GetLatestBlockError` |
| 3 | `SendTransaction(RawTransaction)` | `send_transaction` | `SendTransactionError` |
| 4 | `GetTreeState(BlockID)` | `get_tree_state` | `GetTreeStateError` |
| 5 | `GetBlock(BlockID)` | `get_block` | `GetBlockError` |
| 6 | `GetBlockNullifiers(BlockID)` | `get_block_nullifiers` | `GetBlockNullifiersError` |
| 7 | `GetBlockRange(BlockRange)` | `get_block_range` | `GetBlockRangeError` |
| 8 | `GetBlockRangeNullifiers(BlockRange)` | `get_block_range_nullifiers` | `GetBlockRangeNullifiersError` |
| 9 | `GetTransaction(TxFilter)` | `get_transaction` | `GetTransactionError` |
| 10 | `GetMempoolTx(GetMempoolTxRequest)` | `get_mempool_tx` | `GetMempoolTxError` |
| 11 | `GetMempoolStream(Empty)` | `get_mempool_stream` | `GetMempoolStreamError` |
| 12 | `GetLatestTreeState(Empty)` | `get_latest_tree_state` | `GetLatestTreeStateError` |
| 13 | `GetSubtreeRoots(GetSubtreeRootsArg)` | `get_subtree_roots` | `GetSubtreeRootsError` |
| 14 | `Ping(Duration)` | `ping` | `PingError` |
| 15 | `GetTaddressTxids(TABF)` | `get_taddress_txids` | `GetTaddressTxidsError` |
| 16 | `GetTaddressTransactions(TABF)` | `get_taddress_transactions` | `GetTaddressTransactionsError` |
| 17 | `GetTaddressBalance(AddressList)` | `get_taddress_balance` | `GetTaddressBalanceError` |
| 18 | `GetTaddressBalanceStream(stream Address)` | `get_taddress_balance_stream` | `GetTaddressBalanceStreamError` |
| 19 | `GetAddressUtxos(GetAddressUtxosArg)` | `get_address_utxos` | `GetAddressUtxosError` |
| 20 | `GetAddressUtxosStream(GetAddressUtxosArg)` | `get_address_utxos_stream` | `GetAddressUtxosStreamError` |

All error enums share `GetClientError` (connection failure) as their first
variant. Each has a method-specific second variant wrapping `tonic::Status`.
`SendTransactionError` adds a third variant, `SendRejected(String)`.

## License

[MIT](LICENSE-MIT)

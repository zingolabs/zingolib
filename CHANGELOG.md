# Changelog

All notable changes corresponding to a workspace level to this project
will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Updated `zcash_local_net` and `zingo_test_vectors` to versions supporting Zebra 4.1.0
- `Indexer` trait and `GrpcIndexer` struct in `zingo-netutils` for type-safe indexer communication
- `indexer: GrpcIndexer` field on `LightClient`, owning the indexer connection directly
- `get_trees` as an `Indexer` trait method
### Changed
- `LightClient` methods (`do_info`, `send_transaction`, `sync`) now use `Indexer` trait methods instead of `grpc_connector` free functions
- `LightClient::indexer_uri()` returns `Option<&http::Uri>` via `GrpcIndexer::uri()`
- `LightClient::set_indexer_uri()` delegates to `GrpcIndexer::set_uri()`
- `config.network_type()` calls replaced with `LightClient::chain_type()` where wallet is accessible
- removed `dev` branch reference from `zingo_common_components` crate and pointed to version `0.2`
### Removed
- `grpc_connector` module from `zingolib` — all indexer communication now goes through `zingo_netutils::Indexer`
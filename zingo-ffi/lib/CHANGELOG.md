# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `probe_server(server_uri)` checks an indexer without a wallet and reports
  how far it got as JSON. The `outcome` is `unresolved`, `unreachable`,
  `noAnswer`, `refused` or `verified`, beside the `host`, `port` and
  `resolved` addresses the probe used. `noAnswer` names the bound that fired
  in `after_seconds`, whether the connection or the request ran out of time.
  `unreachable` covers a connection that failed or broke under the request,
  and `refused` is reserved for a status the indexer itself returned. A
  `verified` report carries the server's `details` as an object with the
  fields `LightClient::info` reports. An address without a host is an
  `InvalidInput` error.
- `read_wallet_chain` returns the chain a wallet file was written for
  (`main`, `test` or `regtest`), so a consumer can compare it with the
  server's before opening the wallet.
- `ZingolibError::WalletChainMismatch`: `init_from_bytes` returns it, in
  place of `Init`, when the wallet bytes were written for another chain.

### Changed
- **Breaking:** `get_version` returns zingolib's `zl_` descriptor alone. The
  consumer's `zm_` half, the `ZINGO_MOBILE_DESCRIPTOR` variable and the
  `zm_description` build script are gone; a consumer computes its own
  descriptor and joins the two itself.
- The crate joins the root workspace. Its manifest names the features its old
  workspace carried (`nym` and `perspective` on zingolib, `lightwalletd-tonic`
  on zcash_client_backend, the runtime features on tokio), and its release
  profile is the root workspace's `[profile.mobile]`, which the builder ships.

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `send_swap_deposit(vault_address, amount, memo_hex)` pays a swap deposit that
  carries a memo: a deshield to a reserved wallet-owned transparent address,
  then that address paying the vault with the memo in an OP_RETURN. It sends
  outright and answers `{ txids }` in order, the vault payment last, the shape
  `confirm` answers with. When the deshield was transmitted and the OP_RETURN
  step failed, it answers `ZingolibError::SendAfterDeshield` and the proposal
  stays stored with the deshield's txid: `confirm` resumes it and answers the
  vault payment's txid alone, and proposing again is refused until then.
- `ZingolibError::SendAfterDeshield(String)`, the answer above. Its text
  names the deshield's txid.
- `reserve_refund_address()` reserves the wallet's next refund address and
  reports it in the shape of `create_new_transparent_address`, for a swap
  provider to refund a deposit to.
- `derive_refund_address()` reports that same address without reserving it,
  for a quote to name while the swap is still being chosen: a dropped quote
  leaves the refund index where it was, and the commit reserves it.
- `status_sync` reports `total_blocks`, the block span from the wallet birthday
  to the last known chain height, beside the `total_blocks_scanned` count. The
  `sync_complete` document of `poll_sync` reports `total_outputs_scanned` and
  `total_outputs`, the session's final output totals. Every other key is
  unchanged; both documents are now rendered by
  `zingolib::perspective::sync_progress`.
- The committed `src/exported_surface.txt` pins the exported surface: every
  uniffi item in the crate's sources, the `uniffi.toml` settings, the locked
  uniffi version and the package name. A test fails when the crate and the
  file differ, and a second test fails when a `macro_rules!` body emits uniffi
  items the first test does not expand.
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
- BREAKING: `IndexerConnection.server_uri: String` is now
  `IndexerConnection.indexer: IndexerChoice`, a uniffi enum of `Offline`,
  `Default`, and `Named { uri }`. `Offline` replaces the empty string,
  `Default` attaches to the chain's Default Indexer (mainnet's is
  `https://mainnet.zainod.zingolabs.dev:443`; a chain without one fails
  `Init`), and `Named` carries the URI the string carried before.
- BREAKING: `change_server(serveruri: String)` is now
  `change_server(choice: IndexerChoice)`. `Offline` disconnects the
  session through `go_offline` instead of binding a never-dialed
  placeholder URI, and a `Default` on a chain without a Default Indexer
  fails `InvalidInput` (zingo-adrs zingolib/0063).
- **Breaking:** the bindings come from `#[uniffi::export]` attributes and the
  uniffi derives on uniffi 0.32.2 in place of `zingo.udl`. The UDL, the
  `build.rs` and the crate's `uniffi-bindgen` binary target are gone, and
  `zingo-uniffi-bindgen` generates the Kotlin and Swift bindings in library
  mode from the compiled library. `uniffi.toml` pins the Kotlin load name
  `uniffi_zingo`, so the Android loader does not change. A consumer regenerates
  its bindings; the old generated files do not load the new library.
- **Breaking:** `init_new`, `init_from_seed`, `init_from_ufvk` and
  `init_from_bytes` take one `IndexerConnection` record (`server_uri`,
  `chain_hint` and `sync`) in place of the four loose arguments, and
  `set_config_wallet_to_prod` takes one `SyncSettings` record
  (`performance_level` and `min_confirmations`). `get_latest_block_server` and
  `change_server` keep their UDL argument label `serveruri`, so the Swift label
  stays `serveruri:`.
- BREAKING: moves to the librustzcash NU7 pre-release cohort zingolib
  pins. The viewing key parser lists a P2SH viewing key item as `transparent`
  beside a P2PKH one.
- **Breaking:** `get_version` returns zingolib's `zl_` descriptor alone. The
  consumer's `zm_` half, the `ZINGO_MOBILE_DESCRIPTOR` variable and the
  `zm_description` build script are gone; a consumer computes its own
  descriptor and joins the two itself.
- The crate joins the root workspace. Its manifest names the features its old
  workspace carried (`nym` and `perspective` on zingolib, `lightwalletd-tonic`
  on zcash_client_backend, the runtime features on tokio), and its release
  profile is the root workspace's `[profile.mobile]`, which the builder ships.

### Fixed
- The `mobile` profile of the root workspace builds with `panic = "unwind"`,
  the strategy the standalone zingo-netutils workspace already uses for the
  proxy library. The iOS app links both static archives into one binary, and
  the two copies of std resolve to one panic runtime, so the abort runtime of
  `libzingo.a` ended the app on every panic inside a tokio task of the proxy,
  which Nym's SOCKS5 tasks raise on a closed input channel (#2926). tokio
  now contains such a panic to its task, and UniFFI returns a panic at the
  FFI boundary as an error.
- `poll_sync` returns `Panic` with the panic's message when the sync task
  panicked, instead of re-raising the panic under the client lock and
  poisoning it for the rest of the session.
- The panic hook writes each panic's message, location and backtrace to the
  `log` sink and to stderr, where it stored the report in silence before. On
  iOS the two Binding Layer archives share one std and so one hook, so the
  proxy's panics are logged by it as well. The hook is installed on the
  first exported call of any kind; UniFFI offers no hook at library load.
- The last panic's report lives in a thread-local, so a panic on another
  thread, such as a Nym SOCKS5 task, no longer overwrites the report that the
  `Panic` error of a guarded call carries.
- The recovery contract after `Panic` or `LightclientLockPoisoned`: a panic
  under the client's write guard poisons the lock, every later call fails
  with `LightclientLockPoisoned`, and the next `init_*` call clears the
  poison. A consumer recovers by re-initializing from its saved wallet bytes,
  as a relaunch did under `panic = "abort"`. A lock-discipline test pins the
  sequence.

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `stop_sync()` asks the running session to stop after its next batch, without
  waiting for it; `poll_sync` tells when it has ended. Stopping when nothing
  runs answers "Sync already stopped." rather than failing.
- `sync_caught_up()` answers `{ caught_up, percent }` from the engine's
  published status alone: false with a null percent until the session has
  published one. Unlike `status_sync`, it never reads a stale wallet as
  complete.
- `default_indexer(chain_hint)` answers the chain's Default Indexer as the URI
  text a consumer passes back as `IndexerChoice::Named`, or nothing for a chain
  without one (testnet, regtest). The hint takes the spellings
  `IndexerConnection.chain_hint` takes, and an unknown hint is an `Init` error.
  The value is zingolib's `MAINNET_DEFAULT_INDEXER`, so a consumer no longer
  restates it.
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
- The committed `src/exported_surface.snapshot.txt` pins the exported surface: every
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
- Sync is continuous: wallets are created and loaded with
  `shutdown_on_completion: false`, so a session launched by `run_sync` stays at
  the chain tip and never ends on its own. `poll_sync` answers "Sync task is
  not complete." for as long as it runs; relaunching on that answer is no
  longer needed. Loading overrides the stored settings, so wallets saved by
  earlier releases sync continuously too.
- Wallets are created and loaded with `TransparentAddressDiscovery::default()`
  (gap limit 10) in place of `minimal()` (gap limit 1), wallets saved by
  earlier releases included.
- Every `init_*` stops the replaced client's sync session. Dropping a client
  never ended its engine, which kept scanning against the old server.
- `change_server` stops the running session, which keeps the indexer it was
  launched with; the next `run_sync` uses the new one.
- `status_sync` answers with the status the engine last published, and
  computes it from the wallet only when none has been published.
- The `bucket_index` of a migration window (`migration_status`'s
  `upcoming_windows`, `window_timeline`) numbers the chain's ZIP 318 grid,
  whose boundaries sit three times as many blocks apart from NU7 activation,
  and the window times count 25 seconds a block from there (zingo-adrs
  zingolib/0056, amended). Below the activation nothing changes.
- BREAKING: moves to the librustzcash NU7 release cohort zingolib pins
  (`zcash_client_backend` 0.25.0, `zcash_keys` 0.17.0, `zcash_primitives`
  0.31.0, `zcash_protocol` 0.11.0 and their siblings), which sets the NU7
  mainnet activation height, 3,543,405. The viewing key parser lists a P2SH
  viewing key item as `transparent` beside a P2PKH one, and a shielded
  unified address with a transparent receiver is rendered with that
  receiver.
- The iOS Prebuilt Bundle ships one archive. `Zingolib.xcframework` now
  carries the proxy crate's UniFFI surface beside the wallet's, built under
  the crate's new `zingo-nym-proxy-ffi` feature, and
  `ZingoNymProxyFFI.xcframework` is gone. The app then links one Rust std
  and one `rust_eh_personality` beside its C++ and Objective-C personality
  routines, the three that compact unwind encodes. Two archives brought two,
  and the linker of Xcode 26.3 refused the app's link with "Too many
  personality routines for compact unwind to encode". The `ZingoBindings`
  product, which the app imports, is unchanged.
- BREAKING: `change_server(choice: IndexerChoice)` takes the uniffi enum
  `IndexerChoice` of `Offline`, `Default`, and `Named { uri }` in place of a
  `serveruri: String`. `Offline` disconnects the session through
  `go_offline` instead of binding a never-dialed placeholder URI, `Default`
  attaches to the chain's Default Indexer (mainnet's is
  `https://mainnet.zainod.zingolabs.dev:443`; a chain without one fails
  `InvalidInput`), and `Named` carries the URI the string carried before
  (zingo-adrs zingolib/0063). `get_latest_block_server` keeps its argument
  label `serveruri`, so the Swift label stays `serveruri:`.
- **Breaking:** the bindings come from `#[uniffi::export]` attributes and the
  uniffi derives on uniffi 0.32.2 in place of `zingo.udl`. The UDL, the
  `build.rs` and the crate's `uniffi-bindgen` binary target are gone, and
  `zingo-uniffi-bindgen` generates the Kotlin and Swift bindings in library
  mode from the compiled library. `uniffi.toml` pins the Kotlin load name
  `uniffi_zingo`, so the Android loader does not change. A consumer regenerates
  its bindings; the old generated files do not load the new library.
- **Breaking:** `init_new`, `init_from_seed`, `init_from_ufvk` and
  `init_from_bytes` take one `IndexerConnection` record (`indexer`, an
  `IndexerChoice` where `Offline` replaces the empty server string and
  `Default` attaches to the chain's Default Indexer, failing `Init` on a
  chain without one; `chain_hint`; and `sync`) in place of the four loose
  arguments, and `set_config_wallet_to_prod` takes one `SyncSettings` record
  (`performance_level` and `min_confirmations`).
- **Breaking:** `get_version` returns zingolib's `zl_` descriptor alone. The
  consumer's `zm_` half, the `ZINGO_MOBILE_DESCRIPTOR` variable and the
  `zm_description` build script are gone; a consumer computes its own
  descriptor and joins the two itself.
- The crate joins the root workspace. Its manifest names the features its old
  workspace carried (`nym` and `perspective` on zingolib, `lightwalletd-tonic`
  on zcash_client_backend, the runtime features on tokio), and its release
  profile is the root workspace's `[profile.mobile]`, which the builder ships.

### Removed
- BREAKING: `set_broadcast_candidates`. Migration parts go to the Session
  Indexer like every Transmission (zingo-adrs zingolib/0063), so there is no
  dedicated migration transmission endpoint to set.

### Fixed
- `send_swap_deposit` answers `InvalidInput` for a malformed or a
  non-transparent `vault_address`. It answered `Send`, the class a consumer
  retries.
- The `mobile` profile of the root workspace builds with `panic = "unwind"`,
  the strategy the proxy library already used in its former workspace. The
  iOS app links both static archives into one binary, and
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

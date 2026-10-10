# Binding Layer changelog

The workbench tool `binding-changelog` writes every section below, one per entry
of `bindings/published.toml`, from the lines that the entry's audited crate changelogs
gained after the previous entry's commit, named as `Since`, up to the
entry's commit, which the section heading names. The check in CI
regenerates every section and fails when the committed file differs. The
audited crates of an entry are the two Binding Layer crates and their
direct dependencies in this repository at the entry's commit, as the
publish workflow recorded them from `cargo tree`; each section names them
in its `Audited` line. A change in another crate of this repository appears
only where one of those changelogs records it.

## ba7336710419c9ac1e9362ac1b4e5b0073ff477e

Since 6d0f7dfc05779be7b32b2fc5c61e9543e18e2164.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

##### Added
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

### pepper-sync

- `sync::ScanRange::encloses` and `sync::ScanRange::overlaps`, the geometry of
  a scan range against a block range, and `sync::ScanPriority::in_flight`, the
  priority the wallet holds a selected range at while its scan task runs.
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

### zingolib

- `wallet::expiry`, the transaction expiry delta: `tx_expiry_delta` and
  `tx_expiry_height` answer for a target height, and `NU7_TX_EXPIRY_DELTA` is
  the delta from NU7 activation.
- `testutils::mock_activation_heights` and `mock_activation_heights_with`, the
  era the in-process tests run under: every network upgrade through NU7 at
  height 1. `testutils::mock_indexer::MockChain::new` and
  `SyntheticWalletBuilder` take it in place of `ActivationHeights::default`,
  which leaves NU7 off.
- `utils::system_rng`, the one constructor of the operating system's
  randomness every signing and proving site draws from.
- `wallet::keys::unified::encode_ufvk`, the string encoding of a unified full
  viewing key for a chain.
- Add `data::ServerInfo::from_lightd_info`, the mapping from an indexer's `LightdInfo` to a `ServerInfo` that `LightClient::info` carried inline, so a consumer holding a `LightdInfo` from its own request builds the same record.
- The `netutils` funnel re-exports `TimeoutExpired`, the marker tonic leaves in a status's source chain when the client's own request deadline fired, so a consumer tells a timed-out request apart from a verdict the indexer returned.
- Add `LightWallet::read_chain`, the chain a wallet file was written for, read from the header of version 32 and later files and by a full read under each chain for older ones.
- Add `wallet::disk::ChainMismatch`, carried inside the `io::Error` that reading a wallet file for another chain returns, so the failure can be told apart without its message.
- Add `wallet::keys::WalletKind` and `LightWallet::kind`, the wallet's key material as one of `Mnemonic`, `SpendingKey`, `ViewingKey` with the receivers the key holds, or `NoKeys`, or `KeyError::NoAccountKeys` for a wallet without account zero. zingo-cli and the FFI each computed this themselves.
- A transaction targeting a height at or above the NU7 activation expires 120
  blocks past its target, the delta ZIP 203 and ZIP 218 recommend for
  25-second blocks, where it expired 40 blocks past. Below the activation,
  and on a chain that never activates NU7, the delta stays 40. Every build
  site passes the delta explicitly: sends, the transparent op_return send,
  migration note splitting, and the cap of the offline-signing lift. A
  proposal holding a step shaped like a canonical ZIP 318 crossing leaves the
  expiry to the backend, which gives such a step the ZIP's rolling expiry and
  refuses any other. The canonical expiry of a ZIP 318 migration part is
  unchanged.
- A receiver that sync discovers at an address index keeps the stored
  address's expiry height and expiry time when it is merged into that
  address. The merge rebuilt the address from its receivers alone and dropped
  the metadata, which the wallet's addresses carry none of today.
- The change memo of a send records recipient unified addresses through
  `zingo_memo::create_wallet_internal_memo`, so a ZIP 316 Revision 2 recipient
  is recorded with its revision and metadata in a version 2 memo, and every
  other send keeps writing the version 1 memo earlier releases read. The memo
  no longer records refund address indexes, which no reader consumed. When
  the recipients outgrow the memo field, the memo records those that fit and
  an error is logged naming how many it holds; the send proceeds either way.
- **Breaking:** the Zcash stack moves to the librustzcash NU7 pre-release
  cohort, pinned exactly: `zcash_client_backend` 0.25.0-pre.1, `zcash_keys` 0.17.0-pre.1, `zcash_primitives` 0.31.0-pre.1, `zcash_proofs` 0.31.0-pre.1, `zcash_protocol` 0.11.0-pre.0, `zcash_address` 0.14.0-pre.1, `zcash_transparent` 0.11.0-pre.1, `zcash_encoding` 0.5, `zcash_note_encryption` 0.5, `zcash_script` 0.6, `orchard` 0.16, `sapling-crypto` 0.9, `incrementalmerkletree` 0.9, `shardtree` 0.8, `zip32` 0.3, `bip32` 0.6, `jubjub` 0.11, `secp256k1` 0.33, `rand` 0.10, and `zcash_pool_migration` 0.2.0-pre.1.
  The cohort knows NU7 on testnet (activation height 4,465,026, consensus
  branch id `0x77190AD9`), so a testnet transaction built above that height
  now carries the branch id the network accepts. Mainnet has no NU7 height
  in this cohort. Every type these crates export through zingolib's public
  API moves with them, so a consumer pins the same cohort.
- **Breaking:** `ChainType::activation_height` answers `NetworkUpgrade::Nu7`
  on a regtest chain from `ActivationHeights::nu7`.
- The wallet draws transaction randomness from `utils::system_rng`, the
  operating system's generator unwrapped, where it passed `rand::rngs::OsRng`.
  `rand` 0.10 removed that type and made the system generator fallible.
- `wallet::keys::unified::UnifiedKeyStore` serializes a unified full viewing
  key through `encode_ufvk`, which encodes at ZIP 316 Revision 0 when that
  revision can carry the key and at Revision 2 otherwise.
- A `migration` value transfer now carries the sum of the Ironwood notes the transaction delivered to the wallet, the amount migrated, where it carried the whole self-received sum including any Orchard change. The FFI spliced this value in after the fact; the value transfer now states it directly.
- `lightwallet-protocol` moves to 0.4.0, the upstream rev whose committed
  bindings carry the Ironwood proto fields. No workspace enables
  `rebuild-proto` any longer, so a build of zingolib no longer needs protoc.

### zingo-nym-proxy-ffi

- **Breaking:** the uniffi pin moves from 0.28 to 0.32.2, the root workspace's
  pin, and `zingo-uniffi-bindgen` generates the crate's bindings in library
  mode from the compiled library. A consumer regenerates its bindings; the
  generated files of an older uniffi do not load the new library.

### zingo-netutils

- The crate re-exports tonic's `TimeoutExpired` beside `Status`, the marker
  tonic leaves in a status's source chain when the client's own request
  deadline fired, so a consumer tells a timed-out request apart from a
  verdict the indexer returned.
- BREAKING: `rand` 0.10. `provider::rotation_interval` takes a `rand` 0.10
  generator, and the exit tiers shuffle with the thread-local generator
  where they drew `rand::rngs::OsRng`, a type `rand` 0.10 removed.
- `lightwallet-protocol` moves to 0.4.0, the upstream rev whose committed
  bindings carry the Ironwood proto fields. The workspace no longer enables
  `rebuild-proto`, so a build of zingo-netutils no longer runs protoc.

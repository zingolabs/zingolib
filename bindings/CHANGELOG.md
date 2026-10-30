# Binding Layer changelog

The workbench tool `binding-changelog` writes every section below, one per entry
of `bindings/published.toml`, from the lines that the entry's audited crate changelogs
gained after its `Since` commits, up to the entry's commit, which the
section heading names, each under the headings it sits beneath in its
changelog. The `Since` commits of an entry are the earlier entries that
are ancestors of its commit and of no other such entry (the first entry
names its own). A release line candidate therefore follows the candidate
it fixes, not the last entry, and the first entry after the line merges
back follows both the line's candidate and the last entry of `dev`, with
the lines that neither had. The check in CI regenerates every section
and fails when the committed file differs. The
audited crates of an entry are the two Binding Layer crates and their
direct dependencies in this repository at the entry's commit, as the
publish workflow recorded them from `cargo metadata`; each section names them
in its `Audited` line. A change in another crate of this repository appears
only where one of those changelogs records it.

## 81d799775f11e882896e8710e8af12d6d4d29a41

Since d496bea8c7c476f2dd54f437674f7b4d1cc19b87.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

## d496bea8c7c476f2dd54f437674f7b4d1cc19b87

Since 3fb32d277ef956e75da124c1442526fb4d91d97a.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

#### [Unreleased]

##### Added
- The committed `src/exported_surface.snapshot.txt` pins the exported surface: every
- `preview_send(send_json)`, `preview_send_all(address, memo)` and
  `preview_shield()` answer exactly what `send`, `send_all` and `shield`
  answer, without storing a proposal or pausing sync. They are the calls for
  a screen that shows a fee while the user is still deciding. A proposal
  made only to show a fee held a continuous sync session paused with nothing
  to resume it (zingolib#3104). They answer on the read lock.
- `decline()` drops the proposal `send`, `send_all` or `shield` stored,
  without transmitting it, and lets sync resume: the counterpart of
  `confirm`. It succeeds with no proposal stored. A stored proposal holds
  sync paused until `confirm` or `decline` settles it, and the docs of the
  three proposing calls now say so.

### zingolib

#### [Unreleased]

##### Added
- `LightClient::preview_send`, `preview_send_all` and `preview_shield` create
  the proposal the matching `propose_*` would store and return it without
  storing it or pausing sync, the display-only reads beside `max_send_value`
  for a consumer that shows a fee while its user is still deciding. A consumer
  that proposed only to show a fee left the stored proposal's pause on a
  continuous sync session with nothing to resume it (#3104). The
  `lightclient::propose` module docs now state the contract: a stored proposal
  holds sync paused until the consumer settles it, confirming with
  `send_stored_proposal` or `calculate_stored_proposal`, or declining with
  `clear_proposal`.
- `data::proposal::ProportionalFeeProposal` and `ProportionalFeeShieldProposal`,
  the aliases `propose_send`, `propose_shield` and the previews return, are
  public, so a consumer can name them.

## 3fb32d277ef956e75da124c1442526fb4d91d97a

Since 755c3adaea5906cf55357e69ad1b0a27d746cdb8.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

#### [Unreleased]

##### Changed
- **BREAKING** The private migration's driving functions `plan_ironwood_migration`,
  `start_ironwood_migration`, `continue_note_splitting`, `quick_split`,
  `split_status`, `reschedule_parts`, `window_timeline`, `execute_due_parts` and
  `execute_due_parts_status` are now the methods `plan`, `start`,
  `continue_splitting`, `quick_split`, `reschedule_parts`, `window_timeline`,
  `execute_due_parts` and `progress` of the object `PrivateMigrationDriver`,
  which the module `private_migration_drivers` exports only under the
  non-default feature `unstable`. `progress` answers `{ split, batch }`, the
  two polls in one, since the write lock lets only one of them run at a time. The published Binding Sets are
  built without that feature and no longer carry them; zingo-mobile offers only
  the drain. `migration_status`, `reconcile_migration` and
  `cancel_ironwood_migration` stay, since the mobile client cancels a private
  migration a beta build left in flight.

## 755c3adaea5906cf55357e69ad1b0a27d746cdb8

Since 625f617b63070855d1a4b2a2f61d071cb27c6f82.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingolib

#### [Unreleased]

##### Added
- `ClientConfig::migrated_from` reports the legacy path a wallet file was
  copied from by `ClientConfigBuilder::build`, so a consumer can tell its user.

##### Changed
- `InputSource::select_spendable_notes` on `LightWallet` selects from the
  pools the input selector names alone. In 7.0.0 a request those pools could
  not cover was topped up from Sapling, Orchard and Ironwood in that order,
  and all-funds selection drew on every pool whatever the request named. The
  greedy input selector rejects a note from a pool it did not name, so the
  shortfall now reaches the consumer as insufficient funds.

##### Fixed
- On Linux and other non-macOS, non-Windows desktops, a wallet left in the
  legacy default directory `~/.zcash` is found again. The default moved to the
  user's data directory (`~/.local/share/.zcash`) and nothing carried existing
  wallets over, so a consumer updating past that change failed to open them
  with `ENOENT`. `ClientConfigBuilder::build`, when it uses the default
  directory, copies the named wallet file over from `~/.zcash` (with its chain's
  subdirectory) when it is only there. Only that file is copied, never the
  directory, which zcashd also uses. The copy is staged and renamed into place,
  and the original is kept.

## 625f617b63070855d1a4b2a2f61d071cb27c6f82

Since c060a45041adeeb3f36571bbbdc645a582701bcc.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

#### [Unreleased]

##### Added
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

##### Changed
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
- BREAKING: `change_server(choice: IndexerChoice)` takes the uniffi enum
  `IndexerChoice` of `Offline`, `Default`, and `Named { uri }` in place of a
  `serveruri: String`. `Offline` disconnects the session through
  `go_offline` instead of binding a never-dialed placeholder URI, `Default`
  attaches to the chain's Default Indexer (mainnet's is
  `InvalidInput`), and `Named` carries the URI the string carried before
  (zingo-adrs zingolib/0063). `get_latest_block_server` keeps its argument
  label `serveruri`, so the Swift label stays `serveruri:`.
  `init_from_bytes` take one `IndexerConnection` record (`indexer`, an
  `IndexerChoice` where `Offline` replaces the empty server string and
  `Default` attaches to the chain's Default Indexer, failing `Init` on a
  chain without one; `chain_hint`; and `sync`) in place of the four loose
  arguments, and `set_config_wallet_to_prod` takes one `SyncSettings` record
  (`performance_level` and `min_confirmations`).

##### Fixed
  the strategy the proxy library already used in its former workspace. The
  iOS app links both static archives into one binary, and

### pepper-sync

#### [Unreleased]

##### Changed
- BREAKING: the Zcash stack moves to the librustzcash NU7 release cohort:
  `zcash_client_backend` 0.25.0, `zcash_keys` 0.17.0, `zcash_primitives` 0.31.0, `zcash_proofs` 0.31.0, `zcash_protocol` 0.11.0, `zcash_address` 0.14.0, `zcash_transparent` 0.11.0, replacing their pre-releases. The cohort sets the NU7 mainnet
  activation height, 3,543,405.
- The anchor-checkpoint retention policy runs on the chain's block spacing
  (`zcash_protocol::zip318::BlockSpacing`): from NU7 activation the ZIP 318
  anchor grid's boundaries sit three times as many blocks apart, and the
  retained checkpoints and the retention window, measured in reference
  blocks, follow them.

### zingolib

#### [Unreleased]

##### Added
- `wallet::keys::transparent_address_from_encoded` decodes a transparent
  address the wallet stores encoded, for a network. `zcash_transparent` 0.11.0
  dropped the `TryFromAddress` conversion that did this inline.

##### Changed
- **Breaking:** the Zcash stack moves to the librustzcash NU7 release cohort,
  pinned exactly: `zcash_client_backend` 0.25.0, `zcash_keys` 0.17.0, `zcash_primitives` 0.31.0, `zcash_proofs` 0.31.0, `zcash_protocol` 0.11.0, `zcash_address` 0.14.0, `zcash_transparent` 0.11.0, and `zcash_pool_migration` 0.2.0, replacing their
  pre-releases. The cohort sets the NU7 mainnet activation height, 3,543,405,
  so a mainnet session crosses NU7 on the network's schedule and commits to
  the NU7 branch id `0x77190AD9` above it. Every type these crates export
  through zingolib's public API moves with them, so a consumer pins the same
  cohort.
- A unified address encodes with every receiver it carries, so a shielded
  address with a transparent receiver renders with it again in the address
  listing's `encoded_address` and wherever a wallet address is shown.
  `zcash_keys` 0.17.0 restored that to `UnifiedAddress::encode`; 7.0.0 had
  followed the pre-release, which omitted it.
- A proposal places change only in a pool the account's viewing key receives
  in, with transparent change as a P2PKH output: `LightWallet` answers the new
  `InputSource::get_change_receivers` from the account's unified viewing key,
  and the change strategies consult it. The greedy input selector also
  spends from a later shielded pool when the preferred pool's change alone
  would be dust, where it reported insufficient funds.
- `wallet::expiry::NU7_TX_EXPIRY_DELTA` is `zcash_primitives`'
  `NU7_DEFAULT_TX_EXPIRY_DELTA` and the pre-NU7 delta is its
  `BLOSSOM_DEFAULT_TX_EXPIRY_DELTA`; the values, 120 and 40 blocks, are
  unchanged, and the builder's own default now agrees with the wallet's.
- `data::transaction_request_from_receivers` returns
  `Zip321Error::SproutRecipient` for a Sprout recipient, which ZIP 321
  forbids and `zip321` 0.10.0 refuses.
- **Breaking:** the pool migration schedule follows ZIP 318's NU7 scaling
  (zingo-adrs zingolib/0056, amended). Bucket boundaries are the chain's
  ZIP 318 grid, the multiples of `M` below the NU7 activation and of `3·M`
  from it, the transfer delays are drawn in reference blocks, and the
  canonical expiry grid spans three times as many blocks from activation,
  so every duration keeps its length in time at 25-second blocks. A bucket
  index is now the ordinal of a grid boundary, which equals `height / M`
  below the activation and departs from it above: the new
  `migration::BucketGrid`, built by `MigrationParams::grid` from the chain,
  answers `index_of` and `boundary_of` where `migration::bucket_index` took
  a modulus, and that function is gone; `TransmissionWindow` and
  `WindowReport` keep their `bucket_index` with the new meaning. The window
  times they estimate count 25 seconds a block from activation. A part
  whose window straddles the activation, the last bucket below it, is built
  against the activation height once the chain has crossed it, so its
  transaction commits to the branch the network validates; the heights a
  part was built against travel with the proof as `migration::ProvedPart`
  and are recorded unchanged. `MigrationParams` moves to version 3. A
  migration section written before this release is renumbered at read: an
  index below the activation is unchanged, a window above it that the grid
  does not open moves to the next boundary, and such an anchor is redrawn.
- A part that slipped past its window counts as within tolerance for two
  hours of chain time, 96 reference blocks, rather than 96 blocks.
  (zingo-adrs zingolib/0063), over one transmit target for the whole send:
  the session's live indexer client on nakednet, or one SOCKS5 client over
  the mixnet. The Destination draw, the hedged race across Destinations, and
  the per-Destination Health filter are gone. An Indexerless session refuses
  a send, and a migration part, with `LightClientError::Offline` on every
  wire, where a ready mixnet once carried it to drawn Destinations; a
  bootstrapping or dead mixnet still refuses both with `MixnetNotReady`.

### zingo-nym-proxy-ffi

#### [Unreleased]

##### Changed
- The Binding Layer builder compiles the Android library and the wallet
  library into one target directory, `target/binding-layer/android`, where
  each had its own, so the crates the two share compile once per ABI and sit
  once in the CI cache. The library now lands at
  `target/binding-layer/android/<triple>/mobile/libzingo_nym_proxy_ffi.so`,
  where `android/proxy/` held it. The host plan merges the same way under
  `target/binding-layer/host`. The two cargo invocations stay separate, so
  each crate keeps its own feature set. A workbench test pins the Android
  builds to one directory.
- The crate joins the root workspace, with zingo-netutils and its verifier
  shim. Its own lockfile, `[patch]` stanza and nextest configuration are
  gone; the root lockfile resolves it, and the root CI's nym feature job
  tests it without `live-mixnet` (zingo-adrs zingolib/0011, amended
  2026-10-21).

### zingo-netutils

#### [Unreleased]

##### Changed
- The crate, its verifier shim and the proxy crate are members of the root
  workspace, which holds one lockfile for the wallet and nym stacks; the
  crate's own workspace and lockfile are gone (zingo-adrs zingolib/0011,
  amended 2026-10-21). Members consume the crate with `nym` off; only the
  proxy crate turns it on.

## c060a45041adeeb3f36571bbbdc645a582701bcc

Since 4f04d38c16cb032edffe1823d07bfc75bcf9103b.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

#### [Unreleased]

##### Changed
- The iOS Prebuilt Bundle ships one archive. `Zingolib.xcframework` now
  carries the proxy crate's UniFFI surface beside the wallet's, built under
  the crate's new `zingo-nym-proxy-ffi` feature, and
  `ZingoNymProxyFFI.xcframework` is gone. The app then links one Rust std
  and one `rust_eh_personality` beside its C++ and Objective-C personality
  routines, the three that compact unwind encodes. Two archives brought two,
  and the linker of Xcode 26.3 refused the app's link with "Too many
  personality routines for compact unwind to encode". The `ZingoBindings`
  product, which the app imports, is unchanged.

##### Removed
- BREAKING: `set_broadcast_candidates`. Migration parts go to the Session
  Indexer like every Transmission (zingo-adrs zingolib/0063), so there is no
  dedicated migration transmission endpoint to set.

##### Fixed
- `send_swap_deposit` answers `InvalidInput` for a malformed or a
  non-transparent `vault_address`. It answered `Send`, the class a consumer
  retries.

### pepper-sync

#### [Unreleased]

##### Fixed
- The mempool stream is opened without a call deadline. It was opened with
  the 20 s bound meant for a finite download, sent to the server as
  `grpc-timeout`, so on mainnet almost every stream ended with
  `DEADLINE_EXCEEDED` before the next block, logged a warning, and was
  reopened with the whole mempool resent. The stream now ends when the
  server ends it at the next block, which is the signal the monitor
  already treats as a mined block.
- The finite streams (block ranges, subtree roots, transparent address
  transactions) are bounded at 120 s for the whole download in place of
  20 s. A stall between messages is still caught after 15 s.

### zingolib

#### [Unreleased]

##### Added
- `mixnet::speed::WAVE_WIDTH`, the fixed width of every speed-priority wave,
  which the retired `lightclient::select::SURVEY_WAVE_WIDTH` carried.
- `mixnet::MixnetProxyError::NoSessionIndexer`: the typed refusal of an attach
  or a hosted enable on a session that holds no Session Indexer.

##### Changed
- **Breaking:** `LightClient::attach_mixnet` and
  `LightClient::enable_mixnet_via_host` require a Session Indexer. The attach
  readiness round trip reaches the Session Indexer instead of the census
  member `zec.rocks`, so an Indexerless session refuses typed with
  `MixnetProxyError::NoSessionIndexer` before it dials anything (zingo-adrs
  zingolib/0063).
- Boot proves three exits, not four. The roles are PriceFetch,
  IndexerClient, and Spare, assigned in the order the exits confirm; the
  IndexerSweep role retired with the Server-Selection Sweep (zingo-adrs
  zingolib/0063). An online session whose exit directory offers fewer than
  three exits still refuses the enable.
- **Breaking:** every Transmission and every migration part goes to the Session
  Indexer over the resolved wire, as one pull under the existing retry policy
  (zingo-adrs zingolib/0063). The Destination draw, the hedged race across
  Destinations, and the per-Destination Health filter are gone. An Indexerless
  session refuses a send with `LightClientError::Offline` on every wire, where
  a ready mixnet once carried it to drawn Destinations.
- **Breaking:** `lightclient::indexer_history::FaultDomain` and the attribution
  of a failed attempt move from `destination::health`, which is removed. The
  history no longer folds attempts into a Health.
- `LightClient::probe_destinations` probes the Session Indexer when no target
  is named, in place of the census's mixnet-reachable entries. An Indexerless
  session refuses with `LightClientError::Offline`, and a Session Indexer
  outside the `https` port 443 shape refuses with
  `LightClientError::IneligibleProbeTarget`, as a named target always did,
  where the probe used to answer with an empty list.
- A send builds one transmit target for the whole send, the session's live
  indexer client on nakednet or one SOCKS5 client over the mixnet, where it
  opened a fresh lazy connection to the Session Indexer per transaction.
- A migration part resolves its wire exactly as a send does, so an
  Indexerless session with a bootstrapping or dead mixnet refuses the part
  with `MixnetNotReady` as it refuses a send, where it answered `Offline`.

##### Removed
- **Breaking:** the `zingolib::indexers` and `zingolib::netutils::indexers`
  re-exports of the indexer census, and the `destination::Correspondable`
  implementation for its `Indexer` entry. The census is deleted; the
  Default Indexer table in `zingo_netutils::default_indexer` replaces it,
  and `config::default_indexer_for` reads it (zingo-adrs zingolib/0063).
- `lightclient::select`, the Server-Selection Sweep runner:
  `LightClient::run_server_selection_sweep`, `ServerSelectionError`,
  `SweepProgress`, `SWEEP_HEIGHT_TOLERANCE`, `MAX_SWEEP_EXIT_DRAWS`, and
  `SURVEY_WAVE_WIDTH`; and `mixnet::sweep`, its pure judgment: `Selection`,
  `LiveCandidate`, `SurveyResult`, `RefusalTally`, `SweepError`,
  `live_cohort`, `select`, `healthy_draw_verdict`, `assigned_selection`, and
  `wave_order`. The Session Indexer receives every operation, so no survey
  selects one (zingo-adrs zingolib/0063).
- `destination::Operator`, which only the sweep's one-ticket-per-operator
  draw consumed.
- The `sweep_harness` test target.
- **Breaking:** `destination::servers` (`DestinationServerSet`, `IndexerConfig`,
  `Trust`, `Role`, `Location`, `Transport`, `Draw`, `NoEligibleDestinations`),
  `destination::rotation`, `destination::health`, and
  `destination::same_operator`; the `config::{IndexerConfig, Location, Role,
  Trust}` re-exports; `ClientConfigBuilder::{add_indexer,
  set_remote_indexer_trust, set_migration_transmission_uri}` and
  `ClientConfig::{indexers, remote_indexer_trust, migration_transmission_uri}`;
  `LightClient::{add_indexer, set_destination_servers_for_tests}`; and
  `LightClientError::{NoEligibleDestination,
  MigrationTransmissionTargetIsSyncEndpoint}` (zingo-adrs zingolib/0063).
- **Breaking:** `mixnet::sweep::Selection::transmit_candidates`. The sweep no
  longer computes the cohort minus the sync operator, because no transmitting
  surface draws from it (zingo-adrs zingolib/0063).

### zingo-nym-proxy-ffi

#### [Unreleased]

##### Changed
- The Android library builds under the workspace's `mobile` profile, the one
  profile of the Binding Layer, and the `proxy` profile is gone. The nym stack
  beneath the crate now compiles with the same size optimization and overflow
  checks on both platforms, where the one-archive iOS build had left it checked
  on iOS and wrapping on Android. A plain-operator overflow in a nym crate is a
  panic on both platforms, which ends its tokio task and leaves the process
  running.
- On iOS the crate ships inside `Zingolib.xcframework`, the one archive of the
  `ZingoBindings` package, which the wallet crate builds with its
  `zingo-nym-proxy-ffi` feature. `ZingoNymProxyFFI.xcframework` is gone. The
  app then links one Rust std and one `rust_eh_personality` beside its C++
  and Objective-C personality routines, the three that compact unwind
  encodes. Two archives brought two, and the linker of Xcode 26.3 refused
  the app's link. The `ZingoBindings` product, which the app imports, is
  unchanged.

### zingo-netutils

#### [Unreleased]

##### Changed
- BREAKING: `IndexerChain` lives in `default_indexer`, the table of Default
  Indexers keyed by chain, now that the census module it partitioned is gone
  (zingo-adrs zingolib/0063).
- `Indexer::get_mempool_stream` takes no timeout. The stream is a
  subscription the server ends at the next block, so a call deadline only
  ever cut it short. `GrpcIndexer` sends no `grpc-timeout` on it, and the
  connection it opens on stays bounded by `time::INDEXER_CONNECT_TIMEOUT`.
- `time::HEAVY_UNARY_TIMEOUT` is `time::FINITE_STREAM_TIMEOUT`, 120 s in
  place of 20 s. Every caller was a finite server stream (a block range,
  the subtree roots, a transparent address's transactions) and the bound
  covers the whole download, which the old value cut short on a slow
  server or link. A stall between messages is still caught by
  `time::STREAM_MSG_TIMEOUT`.

##### Removed
- BREAKING: the `indexers` module: the census `Indexer`, `INDEXERS`, the
  uptime leaderboard `MOST_UP_INDEXER_URIS`, `active`, `mixnet_eligible`,
  and `Indexer::operator`. A session attaches to one indexer, the chain's
  Default Indexer or the one the consumer names, so no table of foreign
  endpoints remains to draw from (zingo-adrs zingolib/0063).
- BREAKING: `time::SERVER_RANKING_TIMEOUT`, which only the retired
  nakednet server-selection probe read.
- BREAKING: `live_indexer_discovery` and the `live-indexer-discovery` bin. The
  over-mixnet census survey retired with zingo-adrs zingolib/0063.
- BREAKING: `indexers::MIXNET_HEALTH_INDEXER`. The attach readiness round trip reaches
  the Session Indexer, not a census member.
- BREAKING: `time::TRANSMISSION_HEDGE_INTERVAL`. The send's hedged race across
  Destinations retired with zingo-adrs zingolib/0063.

## 4f04d38c16cb032edffe1823d07bfc75bcf9103b

Since d4ac5772d7fc5d4ca621e57523d28f1b24ac24c5.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

#### [Unreleased]

##### Added
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

##### Changed
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

### pepper-sync

#### [Unreleased]

##### Changed
- `sync::MAX_REORG_ALLOWANCE` is 1000, following zebra's
  `MAX_BLOCK_REORG_HEIGHT`, which zebra 5.2.0 raised from 100 and has kept
  since. A re-org up to 1000 blocks deep is truncated and scanned again,
  where a re-org deeper than 100 blocks cleared the wallet's chain data
  before. The wallet keeps the wallet blocks and, through
  `SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE` and `MAX_SHARDTREE_CHECKPOINTS`,
  the shard tree checkpoints of the last 1000 scanned blocks, and
  transparent address discovery searches the 1000 blocks below the last
  known chain height at the start of a sync session. A wallet whose last
  known chain height is more than 1000 blocks above the server's chain
  height returns `SyncError::ChainError`, where the bound was 100.
- BREAKING: `wallet::traits::SyncShardTrees::update_shard_trees` takes the
  session's chain height and the wallet's highest scanned block as of the
  task's selection, in place of the wallet's highest scanned block at
  processing time. A load's rolling checkpoint window hangs from the chain
  height, so only the loads inside the top window of the chain place it,
  whatever task they belong to. Every load also places a checkpoint at its
  own last block, so a session stopped after it anchors a spend there, and
  the anchor retention boundaries as of that block or the highest scanned
  block when that is higher, so such a session anchors a pool crossing
  too. Before, every load of a task above the highest scanned height placed
  a full window hanging from its own last block, which the next load's
  inserts pruned, and the scan and the update could disagree once another
  task moved the wallet's highest scanned block between a load's scan and
  its update.
- A block hash that disagrees more than `MAX_REORG_ALLOWANCE` below the
  chain height the wallet's scanned state was formed against is a
  different chain: the wallet is cleared and
  `ServerError::ChainVerificationError` returned, as for a re-org deeper
  than the allowance. The depth was measured from the wallet's highest
  scanned block, so a wallet stopped between the loads of a task that met
  such a disagreement attempted a truncation, found no checkpoint at the
  fork, and was cleared with `SyncError::TruncationError` instead.
- Scanning places a shard tree checkpoint only on the blocks the wallet
  will hold one for: the top `SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE`
  blocks of a range scanned above the highest scanned height, the blocks of
  a lower range inside that window, and the anchor retention boundaries
  inside their window. Every other block's checkpoint was built, inserted
  and pruned from each pool's tree for nothing, a fixed cost per block per
  active pool that grew with the ironwood pool. The window was sized by
  `MAX_SHARDTREE_CHECKPOINTS` before, which counts the pinned anchor
  boundaries the rolling budget excludes, so the bottom six blocks of every
  window were checkpointed and pruned as well. The heights come from one function,
  `witness::checkpoint_heights`, which the shard tree update also uses to
  decide the checkpoints it adds for blocks that leave a pool's tree
  unchanged, so the two sides agree. A scan task carries the wallet's
  highest scanned height from its selection for this.

### zingolib

#### [Unreleased]

##### Added
- `LightWallet::pending_op_return_deshield` reports the deshield txid of a
  stored OP_RETURN proposal that waits for `send_stored_proposal` to resume
  it.
- `SendError::OpReturnResumePending`: `propose_send_with_op_return` returns it
  while such a proposal is stored, since a new proposal would drop the one
  the resume needs.
- `testutils::mock_indexer::MockChain::mine_filler_orchard_block` mines a block
  of orchard actions no wallet decrypts, so a scan task over the mock chain
  splits into loads by output count.
- `config::IndexerChoice`, the typed three-way indexer choice `Offline`,
  `Default`, or `Named(uri)`, with `resolve(chain)` answering the URI a
  session attaches to and `config::NoDefaultIndexer` when the chain has
  no Default Indexer. `ClientConfigBuilder::set_indexer_choice` takes it,
  `set_indexer_uri` is now `set_indexer_choice(Named(uri))`, and
  `ClientConfigError::NoDefaultIndexer` is the build-time refusal.
  `config::default_indexer_for(chain)` answers the chain's Default Indexer
  (zingo-adrs zingolib/0063).
- `LightClient::choose_indexer`, which applies an `IndexerChoice` to a
  running session: `Offline` goes offline, `Default` and `Named` replace
  the indexer connection, with `lightclient::ChooseIndexerError` naming
  the missing default or the failed connection.
- The crate re-exports `zingo_netutils::default_indexer`.

##### Changed
- `wallet::legacy::MAX_REORG` follows `pepper_sync::sync::MAX_REORG_ALLOWANCE`
  to 1000, so a legacy witness tree keeps 1000 checkpoints, and
  `LightWallet::clear_all` re-targets only the transactions confirmed more
  than 1000 blocks below the last known chain height.

### zingo-netutils

#### [Unreleased]

##### Added
- `default_indexer`, the Default Indexer per chain: `default_indexer(chain)`
  answers the zingolabs endpoint a session attaches to when the consumer
  goes online without naming one, `MAINNET_DEFAULT_INDEXER`
  (`https://mainnet.zainod.zingolabs.dev:443`) for mainnet and nothing for
  testnet until a zingolabs testnet host exists (zingo-adrs zingolib/0063).

## d4ac5772d7fc5d4ca621e57523d28f1b24ac24c5

Since 502a453fe3329ecded1872f0c856b56f141c8d6a.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

#### [Unreleased]

##### Fixed
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

### pepper-sync

#### [Unreleased]

##### Fixed
- The scan results of a task's loads are processed in block height order.
  The loads of a scan range are scanned concurrently and returned in
  completion order, so a load could be processed before a lower load of the
  same range. When the nullifier map is capped by the performance level,
  every processed load of a range above unscanned ranges is scanned without
  mapping, and a note found in the lower load then recorded the higher load
  as a range whose nullifiers must be re-fetched before the note is known to
  be unspent. That re-fetch only runs once every range below is scanned, so a
  note received at the chain tip stayed unspendable for the whole historic
  sync. A result arriving above an unprocessed lower load of its task is now
  held back until that load arrives, and a worker takes a new load only while
  the scanning workers and held results together stay within the worker pool,
  so the loads in memory never exceed the pool.

### zingolib

#### [Unreleased]

##### Added
- `LightWallet::birthday_adjusted_from` reports the height a wallet was given
  when the wallet raised it to Sapling activation. It lives in memory for the
  process lifetime. `RecoveryInfo` and the FFI viewing-key report carry it as
  `birthday_adjusted_from` when it is set.
- `data::PollReport::from_join` maps a tokio join outcome to a `PollReport`: a
  finished task is `Ready`, a panicked task is `Panicked` with the panic's
  message, and a cancelled task is `Panicked` with tokio's description.
- `data::panic_text` renders a panic payload, `&str` or `String`, as text.
- `LightClientError::SyncPanicked(String)` reports a sync task that panicked
  from `await_sync` and from the launch wait in `sync`.

##### Changed
- **Breaking:** `LightWallet::new` and `LightWallet::read` raise a birthday
  below Sapling activation to the activation height instead of failing, and a
  raise on read marks the wallet for saving. `WalletError::BirthdayBelowSapling`
  is removed.
- **Breaking:** `git_description`, and with it the Binding Layer's
  `get_version`, returns `zl_<crate version>_<sixteen characters>` on every
  commit, where the sixteen characters are the prefix of the Published Commit
  that the Maven version of the Android Prebuilt Bundle also carries. A build
  from a modified tree still appends `_dirty`. The five-character abbreviation
  is gone, and so is the release form `zl_<tag version>` that a `zingolib_v*`
  tag at HEAD produced. A consumer that parses or matches the descriptor must
  accept the new form. The build script and the workbench take the length and
  the abbreviation from one source file, `zingolib/short_commit.rs`.
- **Breaking:** `config::construct_indexer_uri` completes a server string
  without a scheme to `https://`, and a URI without a port to its scheme's
  default port (`:443` for `https`, `:80` for `http`), named by the new
  `config::HTTPS_DEFAULT_PORT`, `config::HTTP_DEFAULT_PORT`, and
  `config::DEFAULT_INDEXER_SCHEME` constants. The old completion to `http://`
  and `:9067` is gone: a bare `host` now reaches the indexer the census
  lists instead of a dead legacy port, and an explicit port is never
  rewritten.
- **Breaking:** `data::PollReport` gains the `Panicked(String)` variant, which
  `poll_sync` returns when the sync task panicked and tokio contained the
  panic. Every exhaustive match on `PollReport` needs an arm for it.
- **Breaking:** `LightClient::start_mixnet_session` returns
  `Result<mixnet::SessionStarted, mixnet::SessionStartError>` in place of
  `Result<(), TransportError>`. `SessionStarted` reports what the driver did:
  `OptedOut`, `Spawned { resolution }` with the `ProxyResolution` the one
  precedence rule chose, or `Attached { socks5_addr }`. `SessionStartError`
  carries the same provenance beside its cause, as `Spawn { resolution,
  source }` or `Attach { socks5_addr, source }`, so a consumer names the path
  and the rung of the precedence that chose it without resolving the path a
  second time.
- **Breaking:** `mixnet::provision::resolve_proxy_path` returns a
  `ProxyResolution`, the chosen path with its `ProxySource` (`Explicit`,
  `Environment`, `Bundled`, or `PathLookup { bundled_dir }`), in place of the
  bare path. `ProxyResolution` renders the path and its provenance through
  `Display`, and the `PathLookup` rendering names every source that was empty.

##### Fixed
- `poll_sync`, the save poll, `shutdown_save_task` and the wallet write no
  longer `expect` on a task's join handle. Under `panic = "unwind"` tokio
  contains a task's panic, the join yields an error, and the old `expect`
  re-raised the panic on the caller's thread, under the FFI's client lock,
  which poisoned the lock for the rest of the session. A panicked save task
  restarts, as a failed one does, and the poll reports the panic as an
  `io::Error`.
- `migrate_to_ironwood` reconciles once more after its last part confirms,
  so the migration it returns reports the `Complete` phase. Reconciliation
  judges completion on the persisted part states, and the pass that promotes
  the parts to `Confirmed` cannot also complete the migration, so the one-call
  returned with the phase still `PartsScheduled`.
- The two-phase migration twin mines the mock chain to the first Ironwood-era
  transmission window before the one-call and two buckets past each block that
  confirms transactions, the spans the state machine requires between the
  activation, a note's confirmation and a part that anchors on it. The twin's
  regtest ancestor was ignored and never ran.

## 502a453fe3329ecded1872f0c856b56f141c8d6a

Since ba7336710419c9ac1e9362ac1b4e5b0073ff477e.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

#### [Unreleased]

##### Added
- `status_sync` reports `total_blocks`, the block span from the wallet birthday
  to the last known chain height, beside the `total_blocks_scanned` count. The
  `sync_complete` document of `poll_sync` reports `total_outputs_scanned` and
  `total_outputs`, the session's final output totals. Every other key is
  unchanged; both documents are now rendered by
  `zingolib::perspective::sync_progress`.

### pepper-sync

#### [0.6.0] - 2026-10-15

##### Added
- `sync::SyncStatus::total_blocks`, the block span from the wallet birthday to
  the last known chain height, the denominator `sync_status` already divided
  by and never reported.
- `sync::SyncStatus::nullifier_retrieval_pending`, whether any scan range still
  awaits nullifier mapping or refetching.
- `sync::SyncResult::total_outputs_scanned`, `sync::SyncResult::total_outputs`,
  and `sync::SyncResult::nullifier_retrieval_pending`, copied from the final
  sync status of the session, so a consumer can derive the session's total
  progress from the result alone.
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

##### Changed
- `sync::sync_status`: the session counts saturate against a stale initial
  sync state, whose previously scanned counts stand at or above the wallet's
  whole span, instead of underflowing. The pool totals and the block span are
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

##### Removed
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

##### Fixed
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

### zingolib

#### [Unreleased]

##### Removed
- **Breaking:** remove `wallet::keys::legacy::extended_transparent` from the public API. The module is crate-private: its `ExtendedPrivKey` and `ExtendedPubKey` hold the 32 secret-key bytes or the 33 compressed-point bytes and the 32-byte chain code the wallet file stores, and each reader validates the scalar or the point through the `zcash_transparent` account key types before the wallet loads. `Debug` prints a BLAKE2b digest of both fields. The derivation API (`KeyIndex`, `with_seed`, `derive_private_key`, `derive_public_key`, `get_ext_taddr_from_bip39seed`), which nothing called, is gone. zingolib no longer depends on `secp256k1`, `hmac` or `sha2` directly.

#### [7.0.0] - 2026-10-15

##### Added
- `perspective::sync_progress`, the sync-progress editorial types.
  `SyncProgress`, built from a `pepper_sync::sync::SyncStatus`, derives the
  total block and output percentages the engine reported before: the exact
  ratio on a percentage scale, clamped, reading complete over an empty span
  once sync has started and one step short while nullifiers await retrieval.
  `SessionReport`, built from a `pepper_sync::sync::SyncResult`, derives the
  session's total output percentage by the same rule. Both render the
  `Display` text and the `json::JsonValue` document pepper-sync rendered
  before, with `total_blocks` added to the status document and the two
  output totals added to the result document.

##### Changed
- `SaplingParams` is a plain struct whose `SPEND` and `OUTPUT` constants are
  the embedded Sapling proving parameters, read by `include_bytes!` straight
  from the parameter cache the build script downloads into. `SaplingParams::get`
  and the `rust-embed` dependency are gone, the build script no longer writes
  into the source tree, and `zingolib/zcash-params/` is no longer a tracked
  stub; delete any stale copy there by hand.
- The Sapling prover is parsed from the embedded parameters on the first
  Sapling proof of a transaction build and dropped with the build. A build
  with no Sapling spend and no Sapling output never parses it, which removes
  about a second from every Orchard-only and Ironwood-only send. A build with
  one parses about 51 MB of parameters and frees them when the build ends.
  Before, every build parsed the parameters, after copying them to the heap.
- **Breaking:** the values of `LightWallet::outpoint_map`, and the scan
  targets `LightWallet::clear_all` and the legacy wallet import add, are
  `pepper_sync::wallet::ScanTarget` with private fields, constructed through
  `ScanTarget::new(block_height, Option<TxId>)` and read through its
  accessors. A wallet file written before the change still loads.

##### Fixed
- `LightWallet::spendable_balance` and note selection require the anchor's
  shard to be scanned as well as the note's. The witness path's right-hand
  siblings are computed from the anchor's shard up to the anchor, so a note in
  a scanned shard could count as spendable while its witness could not be
  built. `InputSource::anchor_computable` checks the anchor's shard, where it
  checked the birthday shard, which is what the backend's canonical crossing
  proposal asks of it. (#2953)

### zingo-netutils

#### [Unreleased]

##### Removed
- `time::test::WALLET_RECORD_LAG_BOUND`, behind `testutils`. The live test
  that bounded the sender's wallet record reaching the mempool retired
  with the regtest removal, and no other test enforced the bound.

#### [6.0.0] - 2026-10-15

##### Added
- `nym_proxy::BootstrapEvent` and the observed lifecycle entries
  `NymProxy::start_observed`, `NymProxy::start_over_observed`, and
  `NymProxy::reconnect_observed`: every acquisition race — the first
  bootstrap and each reconnect after a proxy death — reports its steps as
  typed events naming the Exit Node each concerns, while `start`,
  `start_over`, and `reconnect` remain thin silent renderings over the same
  path, so the two narrations cannot drift. `PullFailed` carries the pull's
  whole typed `zingo_net_diag::NetOpFailure` — the stage, the target, and
  the cause chain — rather than a line rendered for a human, so a
  diagnostics surface renders structure instead of parsing prose.

##### Changed
- `lightwallet-protocol` moves from 0.3.0 to 0.4.0, whose committed
  bindings carry the Ironwood proto fields.

## ba7336710419c9ac1e9362ac1b4e5b0073ff477e

Since 6d037df7a4f4f72f219fbadd8c6fe34d8c8ea97c.

Audited `zingo`, `pepper-sync`, `zingolib`, `zingo-nym-proxy-ffi`, `zingo-netutils`.

### zingo

#### [Unreleased]

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

##### Changed
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

### pepper-sync

#### [Unreleased]

##### Added
- `sync::ScanRange::encloses` and `sync::ScanRange::overlaps`, the geometry of
  a scan range against a block range, and `sync::ScanPriority::in_flight`, the
  priority the wallet holds a selected range at while its scan task runs.

##### Changed
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

##### Fixed
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

#### [Unreleased]

##### Added
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

##### Changed
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

#### [Unreleased]

##### Changed
- **Breaking:** the uniffi pin moves from 0.28 to 0.32.2, the root workspace's
  pin, and `zingo-uniffi-bindgen` generates the crate's bindings in library
  mode from the compiled library. A consumer regenerates its bindings; the
  generated files of an older uniffi do not load the new library.
- The crate joins the zingo-netutils workspace. Its own lockfile, `[patch]`
  stanza and nextest configuration move to that workspace, whose CI job tests
  the crate without `live-mixnet`.

### zingo-netutils

#### [Unreleased]

##### Added
- The crate re-exports tonic's `TimeoutExpired` beside `Status`, the marker
  tonic leaves in a status's source chain when the client's own request
  deadline fired, so a consumer tells a timed-out request apart from a
  verdict the indexer returned.

##### Changed
- BREAKING: `rand` 0.10. `provider::rotation_interval` takes a `rand` 0.10
  generator, and the exit tiers shuffle with the thread-local generator
  where they drew `rand::rngs::OsRng`, a type `rand` 0.10 removed.
- `lightwallet-protocol` moves to 0.4.0, the upstream rev whose committed
  bindings carry the Ironwood proto fields. The workspace no longer enables
  `rebuild-proto`, so a build of zingo-netutils no longer runs protoc.

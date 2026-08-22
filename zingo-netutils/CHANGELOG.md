# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- `conduit::MixnetConduit::in_flight` counts guards rather than references
  to the shared core, so cloning a conduit no longer reads as using it. A
  session holds one conduit and hands clones to every surface, and under
  the previous derivation each of those clones inflated the count, leaving
  a superseded conduit permanently short of `Retired`.

### Changed
- The `socks5-fetch` feature enables neither reqwest's `cookies` nor its
  `json`. A price leg is a single stateless GET against a public quote
  endpoint, so there is no session for a cookie jar to carry, and the fetch
  reads the body as text and hands it to the source's own parser, so it never
  touches reqwest's json surface. Both features came across with the fetch
  when it moved out of zingo-price, where they had been inherited rather than
  chosen. Dropping them takes `cookie`, `cookie_store`, `psl-types`,
  `publicsuffix`, and `time-macros` out of the wallet workspace's graph
  entirely, and leaves this crate's own standalone lockfile gaining `reqwest`
  alone where it had gained seven packages.
- BREAKING: `provider::RotationVerdict` gains a `Never` variant, which a
  platform answers when rotation is ruled out for the session's whole life
  rather than merely postponed. `Defer` used to carry both meanings, so a
  desktop that will never rotate had to say so by repeating a ten-minute
  deferral forever; the wallet now retires its rotation watchdog instead of
  parking it on a cadence whose answer cannot change.

### Added
- The `socks5-fetch` feature and the `socks5_fetch` module: one HTTP request
  carried through a conduit, classified into a typed `zingo_net_diag`
  failure. `ConduitDial::fetch_text` is the entry, and it is the first
  operation a conduit performs rather than describes, so a caller reaches the
  wire by holding the guard instead of by reading an address out of it.
- The classification table's fabricated-input tests, which `docs/agents/net-diag-design.md`
  has mandated since the table was written and which nothing had ever
  supplied. One case reaches each stage the table can produce, and two more
  pin where the implementation diverges from the design's rows: the TLS arm
  fires on chain text without the `is_connect()` conjunct the design
  requires, and it outranks `is_status()`, so a status error whose chain
  names a certificate never reaches `RemoteHttp`.
- `provider::HostedProvider::rotation_verdict`: the host's answer on
  spending a rotation's bootstrap now, reaching the wallet through the
  provider so the host itself stays below the seam.
- `time::CONDUIT_DRAIN_BUDGET` and `time::CONDUIT_DRAIN_POLL`: how long a
  superseded conduit may hold its transport open for work already dialed
  through it, and how often that work is rechecked. The budget is
  `MIGRATION_SUBMIT_TIMEOUT`, the longest bounded operation that work can
  be, so a guard outliving it is a leak rather than slow work.
- `conduit::ConduitDial` and `conduit::ConduitState`: a conduit counts its
  outstanding uses, so a superseded transport retires the moment its work
  drains rather than after a guessed interval (ADR 0048). `ConduitState`
  orders `Serving`, `Superseded`, `Retired`, which is the order a conduit's
  life runs in, and it is derived from the count so retirement cannot be
  claimed while work is outstanding.
- `provider::RotationVerdict` and `ProxyHosting::rotation_verdict`: the
  resource-constrained rotation policy a platform states, since only it sees
  the battery, the foreground state, and the radio (ADR 0048).
  `provider::rotation_interval` draws the randomised cadence, and
  `time::CLIENT_ROTATION_MIN` and `time::CLIENT_ROTATION_MAX` bound it.
- `provider::ProxyHosting`, `provider::HostedProvider`,
  `provider::HostedTransport`, and `provider::HostRefusal`: the mixnet
  provider a platform host supplies, moved down from zingolib (ADR 0046).
  `HostedProvider` holds the supplied host, so the dynamic dispatch a
  host requires stays below the seam and the wallet names a concrete
  type. The provider names no async runtime: its methods block, and a
  caller on a runtime hands them to a blocking thread.
- `conduit::MixnetConduit`: what a wallet holds when it has somewhere to
  send mixnet traffic, asked for by role (ADR 0046). It replaces
  zingolib's `SlotTunnel`, which it also retires the term "tunnel" with.
  Its address accessor stays public until the dialers that take a bare
  `SocketAddr` accept a conduit instead.
- `exit::ExitNodeId` and the Exit Pool, moved from zingolib so the wallet
  crate stops owning Nym's vocabulary (ADR 0046).
- `time::NYM_EPOCH`: one Nym network epoch, the hourly topology rotation
  that bounds how long an observation about an Exit Node stays meaningful.
- `time::OBSERVED_ANNOUNCEMENT_MEAN`, `time::OBSERVED_ANNOUNCEMENT_DEVIATION`,
  and `time::ANNOUNCEMENT_DEVIATIONS`: the measured exit-announcement
  latency the readiness grace is now derived from.

### Changed
- BREAKING: `conduit::MixnetConduit` is no longer `Copy`, and its `socks5`
  accessor is gone. Dialing takes a `ConduitDial` guard from `dial()`, whose
  `socks5` is the only way to reach the address, so a use cannot be made
  without being counted (ADR 0047, ADR 0048).
- `time::EXIT_ANNOUNCEMENT_GRACE` falls from 25 seconds to 7. It was one
  connect attempt plus a hedge interval, a bound chosen without measurement.
  The `birth-trial` workbench tool measured thirty pinned births against
  mainnet on 2026-08-18 and found announcement latency averaging 4637
  milliseconds with a standard deviation of 549, and a slowest sample of
  5604. Seven seconds is the four-deviation figure rounded up to a whole
  second. `time::SPEED_ACQUISITION_DEADLINE` derives from the grace and so
  falls with it, from 285 seconds to 105.

### Removed
- BREAKING: the responsiveness partition is retired. The `Responsiveness`
  trait, the `PrioritiseSpeed` and `PrioritisePrivacy` marker types,
  `ResponsivenessClass`, its wire token, and the proxy binary's
  `--responsiveness` argument are gone; `NymProxy::start` and `start_over`
  lose their type parameter. Every acquisition now races under the one
  hedged launch policy (`arm_race::acquisition_launch_policy`), an
  arm wins by binding, and the child-side Sentinel gate is deleted: proof
  of the bound exit belongs to the layer above the SOCKS5 seam, which
  probes once per birth instead of once per losing arm. The speed class's
  in-race proving starved acquisitions whenever no arm could complete a
  round trip quickly, stalling the session tunnel at `Bootstrapping`
  past 90 seconds in three consecutive measured runs.
- BREAKING: the retirement's residue is gone with it. The `responsiveness`
  module is deleted, with `RESERVATION_CLUTCH_SIZE` and
  `acquisition_launch_policy` re-homed in `arm_race` beside the policy they
  configure; `LaunchPolicy::Saturating`, which nothing outside its own
  tests constructed, is removed; and `NymProxyError::CarriesNothing`, whose
  only mint left with the child-side Sentinel gate, is removed.

### Changed
- `time::PROGRESS_HEARTBEAT_INTERVAL` settles on a ten-second cadence. The
  two-second value was a temporary diagnostic aid for the silent-phase
  reports, and its narration crowded the interactive session it served.
- BREAKING: an acquisition's clutch grows from three Exit Node reservations
  to four, and a racing arm now wins by carrying a round trip rather than by
  binding a socket. Building a mixnet client never contacts the exit, so a
  dead exit won the race as readily as a live one. Under the speed priority
  each arm carries a Sentinel round trip before it can win, and an arm
  whose exit stays silent loses the race.

### Added
- `sentinel` module (with the `socks5-transmit` feature): `probe_sentinel`
  carries an ordinary DNS lookup of a constant name to a reliable public
  resolver through a SOCKS5 tunnel, and reports `ExitEvidence` — whether the
  bound Exit Node carried a round trip at all. A survey uses it to tell an
  exit that carries nothing from indexers that will not answer; binding an
  exit proves neither, because the mixnet client reports success against a
  dead exit. `time::SENTINEL_BUDGET` bounds the probe.


### Changed

- BREAKING: the free functions `send_transaction_via_socks5`,
  `get_lightd_info_via_socks5`, and `transaction_known_via_socks5` are
  replaced by the `Socks5Indexer` struct. `Socks5Indexer::new` groups
  the proxy address, the indexer URI, and the round-trip bound once,
  and the methods `send_transaction`, `get_lightd_info`, and
  `transaction_known` run the operations through one private
  dial-and-bound pipeline. Every operation still opens its own SOCKS5
  tunnel.
- BREAKING: `NymProxy::socks5_addr` returns a `std::net::SocketAddr`
  instead of a `String`. The proxy announces the loopback address it
  bound, so a caller dials the typed address it is handed and never
  parses one out of text.

- BREAKING: `send_transaction_via_socks5`, `get_lightd_info_via_socks5`,
  and `transaction_known_via_socks5` take the SOCKS5 proxy address as a
  `std::net::SocketAddr` instead of a `&str`. The one dial-string
  rendering now happens inside the connector, so callers pass the typed
  address they already hold and never render it.

### Removed

- BREAKING: `live_indexer_discovery::DiscoveredIndexer` carries `tip: BlockId`
  where it carried `info: LightdInfo`, because discovery now probes the tip.
- BREAKING: the `zingo-nym-proxy-ffi` crate and the `uniffi-bindgen`
  helper leave this workspace; zingo-mobile now hosts the mobile UniFFI
  proxy shim in its own `nym-host` workspace (zingo-mobile PR #1251).

### Added

- `Socks5Indexer::get_latest_block`: the `GetLatestBlock` tip fetch through
  the local SOCKS5 proxy, the lightest liveness probe an indexer answers,
  used by the attach readiness round trip and by live-indexer discovery.
- The `responsiveness` module partitions network operations at compile
  time: the sealed `Responsiveness` trait with the `Critical` and
  `NonCritical` marker types, the `ResponsivenessClass` enum with
  `wire`, `parse`, and `launch_policy`, and the
  `RESERVATION_CLUTCH_SIZE` constant.
- `LaunchPolicy::Saturating` launches the full clutch at once; a
  Critical acquisition races under it.
- The proxy binary accepts `--responsiveness <critical|non-critical>`
  and defaults a bare invocation to critical.
- `time::TRANSMISSION_HEDGE_INTERVAL` names the send escalation's
  silence interval, derived as `PER_ATTEMPT_CONNECT_TIMEOUT +
  MIXNET_ROUND_TRIP_BOUND` so a responsive Correspondent's confirmed
  delivery beats the first hedge (ADR 0040).

### Changed

- BREAKING: `NymProxy::start` and `NymProxy::start_with_progress` take
  a `R: Responsiveness` type parameter that names the acquisition's
  responsiveness class.
- BREAKING: the `MAX_PARALLEL_CONNECTS` constant is renamed
  `RESERVATION_CLUTCH_SIZE`; the race width is the clutch of exit
  reservations, never an independent parameter (ADR 0035).
- BREAKING: the `arm_race` planner speaks ADR 0035's pull vocabulary.
  `PullFailure` (was `ArmFailure`) carries an `arm` field (was
  `candidate`), `RaceEvent::PullFailed` replaces `RaceEvent::ArmFailed`,
  `RaceAction::Launch`'s field is `arm`, and
  `RaceAction::SetHedgeTimer` replaces `RaceAction::ArmHedgeTimer`
  (whose "arm" was the verb, colliding with the bandit noun).
  `RaceState::new` names its first parameter `arms`.
- BREAKING: an acquisition races a Clutch the parent draws, never a crawl
  it filters itself. `NymProxy::start_over` replaces
  `start_with_progress` and takes the drawn exits; `NymProxy::start`
  draws its own for a standalone run. The proxy binary takes repeated
  `--exit <identity>` arguments in place of `--exclude-exit`, and gains
  `--discover`, which prints the directory's Exit Nodes and exits — the
  parent's one window onto a population it cannot query itself.
  `MAX_EXIT_NODE_ATTEMPTS` and `NymProxyError::AllExitsExcluded` are
  removed, the clutch being the race's whole width.
- BREAKING: the census has no default server. `DEFAULT_INDEXER_URI`,
  `DEFAULT_INDEXER_URI_TESTNET`, the `Indexer::default` field, and
  `default_uri` are removed; a session either pins a server explicitly
  or lets the Server-Selection Sweep select one.
- BREAKING: the spawned binary's health gate is deleted. The bound Exit
  Node and the SOCKS5 address are announced at bind time, end-to-end
  verification belongs to the session's sweep, and `MIXNET_HEALTH_DRAWS`
  and `indexers::MIXNET_HEALTH_INDEXER` are removed.
- BREAKING: the periodic probe convention is retired. Attach readiness
  and the recurring check are loopback dials only; the timing constants
  rename accordingly: `LISTENER_MONITOR_INTERVAL` (was
  `LIVENESS_PROBE_INTERVAL`), `ATTACH_WATCHDOG_INTERVAL` (was
  `ATTACH_PROBE_INTERVAL`), and `ATTACH_LISTENER_RETRY_PAUSE` (was
  `ATTACH_HEALTH_RETRY_PAUSE`), and `ATTACH_READINESS_BUDGET` retunes
  from 61 s to 11 s.
- BREAKING: the responsiveness classes are renamed for the tradeoff they
  declare: `PrioritiseSpeed` (was `Critical`, saturating) and
  `PrioritisePrivacy` (was `NonCritical`, hedged), across the marker
  types, the `ResponsivenessClass` variants, and the wire tokens
  (`--responsiveness <prioritise-speed|prioritise-privacy>`). A class
  names the acquisition's declared priority, never who waits.
- BREAKING: the Exit Node vocabulary replaces "provider" throughout the
  proxy API (ADR 0038's glossary; "provider" is Loopix's word for the
  gateway role, a false friend). `NymProxy::exit_node`,
  `NymProxy::start_with_exit_node`, and `NymProxy::discover_exit_nodes`
  replace `exit_provider`, `start_with_provider`, and
  `discover_exit_providers`. `NymProxyError::NoExitNode` replaces
  `NoProvider`, and the `AttemptTimeout` and `AttemptsExhausted`
  `Display` renderings now say "exit node" where they said "provider".
  `DiscoveredIndexer` and `DiscoveryFailure` carry `exit_node` (was
  `exit_provider`).

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

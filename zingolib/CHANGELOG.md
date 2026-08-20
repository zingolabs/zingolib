# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- BREAKING: `mixnet::resolve_route` takes the session's
  `MixnetConduit` rather than a `SocketAddr`, and `LightClient::mixnet_route`
  reads it from the new `LightClient::mixnet_conduit`. The resolver used to
  mint a fresh conduit per call, which gave every surface a private count
  and left a rotation with nothing to supersede; one conduit per attached
  client is what makes the supersession reach the work (ADR 0048).
- A session with an attached Standing Client now runs a rotation watchdog
  beside its proof watchdog: on a randomised interval between
  `CLIENT_ROTATION_MIN` and `CLIENT_ROTATION_MAX` it asks the acquirer
  whether the platform can afford a bootstrap, and where the answer is
  `Now` it hands the session to a freshly proven client (ADR 0048). The
  hand-off is make-before-break, so a rotation that cannot prove a
  replacement keeps the incumbent and publishes nothing. A desktop's
  spawned acquirer answers `RotationVerdict::Never`, which retires the
  watchdog before it waits at all and leaves ADR 0045's four role-bound
  clients unchanged.
- A failover that installs a replacement now retires the client it replaced
  rather than stopping it: the outgoing conduit is superseded so it takes no
  new work, its transport stays open until the work already dialed through
  it finishes, and only then does the child stop. `MIGRATION_SUBMIT_TIMEOUT`
  bounds the wait, past which a held guard is treated as leaked.
- BREAKING: `wallet::migration::parts::ProveOnce` is a named struct rather
  than a boxed `FnOnce`, and `PrepareResult::Ready::prove` holds a
  `Box<ProveOnce>`. Call `prove.prove()` where the closure was invoked.
  Naming the type states what the proving work owns, which the closure
  left implicit.
- BREAKING: `mixnet::MixnetConduit` loses its `socks5` accessor and its
  `Copy`. A surface holding a route now takes a `ConduitDial` guard and
  holds it for the work's duration, which is what lets a superseded conduit
  retire once its work drains (ADR 0048). `mixnet::speed::SpeedPrioritized`
  races a conduit rather than an address, and `MixnetTransmissionClient`
  holds a guard for its life because it dials on every submission.
- BREAKING: `mixnet::acquire::TransportAcquirable` gains `rotation_verdict`,
  which the rotation watchdog asks at the moment of acting rather than when
  it drew its cadence, because the battery and foreground state a host
  weighs are the ones it has then.
- BREAKING: `mixnet::acquire::ProxyHosting` gains `rotation_verdict`, which a
  platform answers to state whether it can afford a rotation now (ADR 0048).
  `MixnetTiming` gains `client_rotation_min` and `client_rotation_max`, so a
  host reads the cadence bounds rather than pinning its own copy.

### Removed
- BREAKING: `mixnet::acquire::ProxyHost` is renamed `ProxyHosting` and moved
  to `zingo_netutils::provider`, along with `HostedTransport` and
  `HostRefusal`; all three are re-exported from `mixnet::acquire`.
  `HostedProxy` is replaced by `zingo_netutils::provider::HostedProvider`.
  `LightClient::enable_mixnet_via_host` takes `impl ProxyHosting` by value
  rather than `Arc<dyn ProxyHost>`, so no caller names a trait object
  (ADR 0046).
- BREAKING: `mixnet::SlotTunnel` is replaced by
  `zingo_netutils::conduit::MixnetConduit`, re-exported as
  `mixnet::MixnetConduit` (ADR 0046). Its `addr` and `into_addr`
  accessors become `socks5`. `mixnet::ExitNodeId` keeps its path as a
  re-export, but is now defined in `zingo_netutils::exit`.

### Changed
- **Breaking.** The Zcash stack moves to `zcash_primitives` 0.30, `zcash_proofs`
  0.30, `zcash_transparent` 0.10, `zcash_keys` 0.16 and `zcash_client_backend`
  0.24.0-rc.7, and `zcash_pool_migration` moves from a git revision to the
  published 0.1.0-rc.7. These types cross zingolib's public API, so a consumer
  must move with them. Sourcing the migration crate from the registry collapses
  the second `zcash_primitives` that its git revision used to drag in, so the
  workspace now compiles one copy and the migration schedule no longer insulates
  heights across a version divide.
- **Breaking.** ZIP 318's ratified constants now live in `zcash_protocol::zip318`,
  and `ANCHOR_AGE_CAP` moves from 16 boundaries to 4. `MIGRATION_MAX_DENOMINATION_ZEC`
  and `RESIDUAL_MIGRATION_MIN` are renamed `DENOM_CAP` and `MAX_RESIDUAL_VALUE`,
  and both are `Zatoshis` rather than a count of whole ZEC. The transfer-delay
  mean moves from 144 blocks to 66, halving the average wait before a migration
  transfer broadcasts. `MigrationParams` keeps version 2, because neither the
  anchor cap nor the delay mean feeds the consent hash.
- The Server-Selection Sweep probes and assigns every indexer before its
  verdict (ruled 2026-08-14): no healthy answer ends the survey early, a
  healthy pinned server still wins outright, and otherwise the sync
  indexer is drawn among every healthy answer, all of which are
  draw-eligible, with the transmit candidates excluding the sync
  operator so different operations select different indexers.
- **Breaking.** `MAX_DIARY_ATTEMPTS` is now `MAX_HISTORY_ATTEMPTS`. The term
  _Indexer Diary_ is retired: a diary was something the wallet kept, and the
  history it names now ends with the session.
- A price fetch no longer writes to the wallet. `update_current_price` used to
  record the quote into the wallet's price list and set `save_required`, so
  asking the price dirtied the wallet and provoked a save; the price now lives
  only in the returned `MixnetPriceFetch`. The price list is still serialized,
  so the wallet format is unchanged, and nothing loses a reader — the only
  consumers of the stored price were already commented out.
- **Breaking.** `mixnet::MixnetMode` is now `mixnet::Indicator`, its parse
  refusal `UnknownMixnetModeToken` is now `UnknownIndicatorToken`, and
  `LightClient::mixnet_mode` is now `LightClient::read_mixnet_indicator`. An
  indicator reports which one of a closed set of states holds, which is what
  the type does and what _mode_ never said.

### Deprecated

### Added
- The session keeps a `NodeHealthIndex`: one epoch-scoped observation per
  Exit Node, written only by the Exit Pool's own acquisition and recycle
  paths. A `Proven` observation (a completed round trip: a Sentinel answer
  or a carried task) is trusted for one Nym epoch; a `Failed` observation
  (a refusal, a timeout, or silence past budget) stands for the session.
  Clutch draws sample fresh-Proven exits first, unknown ones next, and
  Failed ones only at exhaustion.
- Every mixnet client is a Proven Client: an acquisition whose bound exit
  carries no trusted fresh proof must answer the Sentinel before its first
  use, a refusal condemns the exit and births a successor, and an
  acquisition whose every birth failed its proof refuses with the new typed
  `TransportError::NoProvenExit`.
- A spawned `nym-proxy` that dies before speaking its stdout protocol now
  latches a typed `proxy-launch` death detail (new `NetOpStage::ProxyLaunch`)
  naming the binary, the launch arguments, and the child's stderr tail, so a
  version-skewed older binary is diagnosed instead of reported as a bare
  death.
- `lightclient::LightClient::from_bytes` constructor — creates a `LightClient` by
  deserializing wallet bytes from memory via `std::io::Cursor`, without reading any file.
  Intended for mobile platforms (iOS/Android) where the native layer owns all file I/O
  and passes the raw wallet bytes across the FFI boundary. Restores the in-memory
  construction path that was lost when `create_from_wallet` and `WalletBase` were removed
  in 5.0.0; the new path uses the `WalletConfig` enum and the existing
  `LightWallet::read` deserializer, so consumers don't need a `Read` variant from a path.
- ZIP 318 Orchard to Ironwood migration, in `lightclient::migrate`:
  - Immediate path: `plan_immediate_migration`, `quick_immediate_migration`.
  - Note splitting, stateless and one round per call: `plan_note_split`, `quick_split`.
  - Scheduled path: `plan_ironwood_migration`, `start_ironwood_migration`,
    `execute_due_parts`, `auto_transmit_if_due`, `reconcile_migration`,
    `catch_up_migration`, `reschedule_parts`, `cancel_ironwood_migration`.
  - Reporting: `migration_status`, `window_timeline`, and the
    `split_progress_handle` / `batch_progress_handle` progress handles.
- `wallet::migration`: plans, parts, denominations, buckets, schedule, persisted state.
  - A Part carries two independent buckets: `bucket_index`, the window it is
    transmitted in, and `anchor_bucket`, the lower bucket whose boundary it proves
    against. `schedule::AnchorFloor` resolves the two floors a candidate anchor
    must clear (strictly above the NU6.3 activation bucket; at or above the
    boundary covering the Part's own bound note), and `draw_anchor_bucket`
    reject-samples an age from `draw_anchor_age` against them.
  - The anchor age is drawn per Part, `Geometric(1/2)` capped at
    `schedule::ANCHOR_AGE_CAP`, and is never zero, so a Part never proves against
    the boundary of the window it is still inside (the ZIP 318 anchor-age draw;
    ADR 0018). The builder's target height, and so the consensus branch the Part
    commits to, comes from the transmission window instead.
  - Consequence for consumers: a wallet that schedules immediately after note
    splitting waits one extra window (~3h at `M` = 144) before its first Batch is
    due, because a fresh note floors the anchor at the next boundary and a legal
    window sits a bucket above its anchor. A wallet whose notes confirmed at least
    one bucket earlier has its first Batch due the moment it is scheduled. Read the
    wait from `MigrationStatus::upcoming_windows`, whose `TransmissionWindow`s carry
    `window_opens_unix_time`, rather than assuming a Batch is immediately sendable.
  - The migration section of the wallet file carries its own version, independent
    of the wallet format version, and ships at 4.
- `nym` module: Nym mixnet transport, behind the new off-by-default `nym` feature.
  Migration-part transmissions route by Mixnet Mode and never at the sync host.
- `nym-diary` feature: per-indexer diary, a per-session runtime opt-in, capped and sanitized.
- Ironwood pool in summaries: `ironwood_notes`, `outgoing_ironwood_notes`,
  `is_orchard_to_ironwood_migration`.

### Changed
- A bind-stage failure spends a proving birth instead of escaping the
  acquisition. A Clutch that never produced a ready bound transport —
  the readiness budget missed, the child dead mid-bootstrap, the status
  channel closed — convicts its drawn exits in the NodeHealthIndex and
  a fresh Clutch is drawn, up to the six-birth budget; only the
  environment's own refusals (a missing binary, an unreachable host, an
  unseeded or exhausted pool) still abort at once, and a defective exit
  report retries without convicting the exits it failed to name.
  Previously one 120-second `NotReady` aborted the whole acquisition
  fatally with nothing learned, and the unbootstrappable exits stayed
  eligible for the very next draw.
- The F1 demotion loop lands whole. An exit-implicating failure on the
  Standing Client — a failed mixnet transmission, or a correspondent
  probe wave nobody answered — raises a suspicion that spawns a
  ProofAcquisition: one arbiter Sentinel exchange dialed into the
  client's tunnel. An answer promotes and refreshes the exit's
  EpochProven observation; silence convicts the exit
  (`ExitNodeHealthVerdict::Failed`) and runs the two-layer failover —
  the mode dips to `Bootstrapping` while a replacement Proven Client
  births over a preference-ordered draw, `Ready` on success, `Died`
  latched when every birth exhausts or no acquirer exists to rebirth
  from. An expiry watchdog fires the same ProofAcquisition unprompted
  the moment the client's proof stops being epoch-fresh, a trusting
  birth inheriting the stale observation's original expiry as its
  deadline. The slot moved behind a mutex so the loop runs from the
  operation paths that observe the failures.
- BREAKING: `MixnetMode` gains a sixth state, `PreviouslyProvenThisEpoch`
  (wire token `previously_proven_this_epoch`), adjacent to `Ready`: the
  Standing Client is up on stale proof — born trusting an EpochProven
  observation an earlier client earned — and no round trip of its own has
  yet confirmed the exit. It routes exactly as `Ready`; the first
  confirmed round trip (a delivered mixnet transmission or an answered
  correspondent probe) promotes it to earned `Ready` and refreshes the
  exit's EpochProven observation. Consumers matching `MixnetMode`
  exhaustively, including mobile's FFI mapping, must add the state.
- BREAKING: the session's standing client is born as a Proven Client.
  `enable_mixnet` and `enable_mixnet_via_host` lose their responsiveness
  type parameter and return once the client is bound and its exit proven,
  instead of returning while an unproven transport bootstraps; the slot
  holds only the bound exit's lease rather than the whole Clutch, and
  `ProxyHost::start_transport` loses its class parameter. The retired
  speed-class enable race stalled at `Bootstrapping` past 90 seconds in
  three consecutive measured runs; the proven birth reached `Ready` and
  quoted prices on its first live run.
- BREAKING: the Correspondent Pools' member-keeping is retired. Go-online
  no longer launches background refills, the Indexer and Price complements
  are gone, and a Transmission's pulls multiplex over the session's standing
  tunnel instead of consuming per-pull Exclusive members; the price fetch
  keeps its own per-run Proven Client, so priced traffic never shares an
  egress with wallet-correlated streams. The go-online refills measurably
  contended with the scan: knocking them out returned a 5,000-block sync
  from 80.3–82.3 seconds to 70.8–73.3 under identical conditions.
- BREAKING: the Sentinel leaves the survey and price waves. Proof belongs
  to a client's birth, so waves run at full indexer width, and the redraw
  survives as the safety net keyed on a wave that not one target answered.
- BREAKING: the redraw of a dead Exit Node serves every speed-priority
  operation. `mixnet::speed::run_speed_prioritized` owns the loop an
  operation used to carry itself: acquire a transport, run the wave, and on
  a Sentinel's silence hold the dead exit until its replacement binds so the
  pool cannot offer it again, up to `MAX_SPEED_EXIT_DRAWS`. An operation
  supplies `acquire`, `dispose`, and `narrate`; the price run gains the
  redraw the sweep already had, and both dispose of a spent transport in the
  background rather than making a caller wait on teardown. Measured over
  twenty live rounds each: the price run failed nine of twenty before, and
  none of twenty after, at a mean of 7.9 seconds against about 5; the sweep
  is unchanged at twenty verdicts of twenty and a 9.5-second mean.
- BREAKING: `lightclient::select::ServerSelectionError` falls from seven
  variants to two, `Speed` and `Selection`. Four of the removed variants
  renamed failures `mixnet::acquire::TransportError` already carried, and
  `ProxyStart` and `ExitOutsideClutch` were among them; a consumer matching
  on those now matches `Speed`, whose source chain carries the transport's
  own error. `wallet::error::PriceError` likewise replaces
  `TransportAcquisition` and `ExitCarriesNothing` with `Speed`.
- `mixnet::acquire::TransportError::DiedDuringBootstrap` carries its death
  detail as a `#[source]` rather than formatting it into the message, so a
  caller reaches the typed `zingo_net_diag::NetOpFailure` whole. Its message
  no longer repeats the detail.
- BREAKING: one wave serves every speed-priority operation. The new
  `mixnet::speed` module holds `SpeedPrioritized` — an operation's targets,
  how it probes one, and what settles it — and `run_wave`, which opens
  `lightclient::select::SURVEY_WAVE_WIDTH` lanes with a Sentinel holding one
  of them, ends the moment the operation settles, and abandons the whole
  wave when the Sentinel proves the Exit Node carries nothing. The
  Server-Selection Sweep and the price run are its two implementations, so
  the width, the Sentinel, and the abandonment rule have one definition
  rather than one apiece. `lightclient::select::survey_tunnel_width` is
  replaced by the `SURVEY_WAVE_WIDTH` constant: the width counts
  connections through the one Nym client, which does not vary with how many
  targets there are.
- BREAKING: `wallet::error::PriceError` gains `ExitCarriesNothing`, which a
  price run returns when its exit carried no round trip. The run previously
  reported every source as having timed out, charging nine operators for a
  tunnel that reached none of them.
- BREAKING: a speed-priority survey now carries a Sentinel in its opening
  wave and restarts on a dead exit. The Sentinel is a reliable public
  address, probed with an ordinary DNS lookup through the same tunnel; it
  holds one of the wave's lanes rather than adding one, because the survey
  width is a ceiling measured for the one Nym client a mobile host runs
  in-process. Its silence within `zingo_netutils::time::SENTINEL_BUDGET`
  proves the Exit Node carries nothing, whereupon the sweep abandons that
  exit — holding its reservation until a replacement binds, so the pool
  cannot offer it again — drops every result of the failed attempt, and
  surveys afresh, up to `lightclient::select::MAX_SWEEP_EXIT_DRAWS` exits.
  `SweepProgress` gains `ExitAbandoned`. Nothing from an abandoned draw
  charges any indexer's Health: a tunnel-phase failure is the exit's.
- A survey whose opening wave times out to a leg now ends there. Every leg
  timing out is evidence about the tunnel rather than about the candidates,
  which the remaining waves can only repeat at the same cost, so a dead
  sweep exit refuses the cohort after one wave instead of after all of
  them. The new `mixnet::sweep::opening_wave_timed_out` states the reading.
- A transport failure whose text says its deadline has elapsed now
  classifies as a timeout. Tonic reports an exhausted leg budget that way,
  and the classifier matched only the other spellings, so genuine timeouts
  reached the sweep's cause tally and the indexer history as `other`.
- BREAKING: the sweep offers the first healthy indexer immediately. The
  survey assigns candidates to lanes at random with the pinned server
  guaranteed an opening lane, opens at most
  `lightclient::select::survey_tunnel_width(candidates)` tunnels at once —
  a bounded function of the census, calibrated for the one shared Nym
  client every platform hosts — and binds the first healthy answer as the
  sync indexer the moment it arrives, the pin preempting while its own
  probe is pending. Every unresolved candidate continues in the background
  as the health sweep, whose handle the session holds: `go_offline` aborts
  it, so revoking consent stops all networking. A pinned session whose pin
  did not answer binds the first healthy alternative and says so. The
  runner no longer draws from a median-judged cohort and no longer fails
  `DeadPin` (the accepted tradeoff: random lane assignment already forces
  an adversary to be lucky).
- BREAKING: `lightclient::LightClient::switch_on_mixnet_for_tests` takes a
  `std::net::SocketAddr` instead of a `&str`. The helper used to parse the
  text and abort the process on a placeholder, which killed an external
  harness. The contract is now checked at compile time. A caller passes a
  parsed address, so `"127.0.0.1:1"` becomes `"127.0.0.1:1".parse().unwrap()`
  or an address constant of its own.
- The indexer diary tolerates a corrupt exit column per column. A stored row
  whose exit column no longer names an Exit Node now loads with every other
  field intact and no exit, where it was previously dropped whole.
- BREAKING: `lightclient::LightClient::shutdown_save_task` returns
  `std::io::Result<SaveShutdown>` instead of `std::io::Result<()>`, where the
  new `lightclient::SaveShutdown` enum distinguishes a stopped task
  (`ShutDown`) from an absent one (`NotRunning`), so callers can report a
  shutdown request against a never-launched saver accurately.
- BREAKING: `lightclient::select::ServerSelectionError` gains the
  `ExitOutsideClutch` variant, which carries the exits the ready transport
  reported. The bind refusal previously reached callers wrapped in
  `TransportAcquisition`, whose message names a Clutch that could not be
  drawn, and an exhaustive match over the enum needs the new arm.
- BREAKING: `lightclient::LightClient::attach_mixnet` now refuses an empty
  exit report. `mixnet::MixnetProxyError` gains the `NoExits` variant, which
  the attach returns when the host names no bound Exit Node, and an
  exhaustive match over the enum needs the new arm. A host that attaches
  must name the Exit Node its proxy bound, so Ready means the address and a
  bound exit at every door.
- The readiness gate now bounds its wait for the transport's first Exit Node
  announcement with the new `zingo_netutils::time::EXIT_ANNOUNCEMENT_GRACE`,
  which runs from the moment the address arrives. A proxy that latches ready
  and never announces a usable exit refuses within the grace instead of
  holding the go-online moment for the whole `NYM_LIFECYCLE_TIMEOUT`. The
  refusal is the existing `NotReady` variant, carrying the grace as the
  budget it exceeded.
- BREAKING: the survey reports its refusal causes.
  `mixnet::sweep::SurveyResult` gains a `refusal` field carrying the diary's
  `FailureKind`, and `SweepError::EmptyCohort` gains a `causes` tally, so the
  refusal itself says whether the transport or the indexers failed, e.g. "0 of
  17 answered, none within the cohort (17 timeout)". A saturated transport now
  reads differently from dead indexers without any external harness.
- `zingo_netutils::Socks5Indexer::get_latest_block` fetches a candidate's tip
  through the proxy, the lightest liveness probe an indexer answers. The attach
  readiness round trip and the live-indexer discovery probe use it. The
  Server-Selection Sweep's own survey keeps `get_lightd_info`, because the
  first-healthy verdict rests on the chain a candidate names and a tip carries
  no chain identity.
- BREAKING: `mixnet::acquire::TransportError` gains the `ExitOutsideClutch`
  variant. A transport that reports ready without announcing an exit from
  the drawn Clutch now refuses with this variant instead of panicking, and
  an exhaustive match over the enum needs the new arm.
- BREAKING: a refused Server-Selection Sweep names its transport failure by
  type. `lightclient::select::ServerSelectionError::TransportUnready(String)`
  is replaced by three variants: `TransportDied`, which carries the death
  report's typed `zingo_net_diag::NetOpFailure` as a `source()` link;
  `TransportTimeout`, which carries the bootstrap budget that elapsed; and
  `TransportStatusClosed`, for a status channel that closed before
  readiness. A caller now distinguishes a handshake-stage death from a
  bootstrap timeout by matching, never by parsing prose.
- BREAKING: a failure detail lives in exactly one chain link. The
  wrapper variants of `LightClientError` and
  `PriceError::TransportAcquisition` stop embedding their sources' text
  and carry them as `source()` links (the pure `{0}` wrappers become
  transparent), because every consumer walks the chain;
  `TransportError::HostRefused` and the wrapper variants of
  `lightclient::error::SendError` and `wallet::error::PriceError` follow
  the same rule.
  `LightClientError::NoEligibleCorrespondent` now carries the typed
  `correspondent::NoEligibleCorrespondents` union — that union and
  `correspondent::Operator` are public — so the empty-pool and
  all-excluded stories reach consumers distinctly. The mixnet refusals
  prescribe `network on` / `network off`, the commands that exist.
- BREAKING: the `zingolib::nym` module is renamed `zingolib::mixnet`,
  per the seam rule (ratified 2026-08-11): above the local SOCKS5 seam
  the wallet's transport domain speaks "mixnet", while "Nym" stays the
  name of the vendor stack below it (the `nym` cargo feature, the
  `nym-proxy` binary, and the netutils Nym types are unchanged). Every
  `zingolib::nym::…` import becomes `zingolib::mixnet::…`; no type,
  function, or variant is renamed.
- BREAKING: mixnet Exit Node identities travel as the typed
  `mixnet::ExitNodeId` instead of bare strings — in `MixnetStatus::exits`,
  `LightClient::attach_mixnet`, `ProvisionStrategy::Attach`,
  `HostedTransport`, and the `ProxyHost` seam — and the operator key
  behind Correspondent exclusion is the typed `correspondent::Operator`
  (String-promotion census of the ADR 0041 arc). Construction is checked:
  `ExitNodeId::parse` (and `TryFrom<String>`, which deserialization uses)
  trims the candidate and refuses a blank with the typed
  `BlankExitNodeId`, so no blank identity can enter the Ready snapshot or
  the Exit Pool ledger. The serialized `MixnetStatus` wire is a plain
  string, unchanged; a malformed exit entry on that wire now refuses
  deserialization as suspicious, because producer and consumer are
  pinned to one code revision.
- BREAKING: the `ProxyHost` seam speaks types end-to-end — its
  `start_transport` takes the `ResponsivenessClass` enum instead of the
  wire string, and both host methods refuse with the typed
  `mixnet::acquire::HostRefusal` instead of `String`, which
  `TransportError::HostRefused` now carries as its source.
- BREAKING: indexer endpoints are the typed `correspondent::Host` — the
  Health ledger's key, `MixnetProbe::host`, and the diary's
  `IndexerAttempt::host`, whose `exit` field is now the typed
  `mixnet::ExitNodeId`. A Host is lowercased at construction because DNS
  names compare case-insensitively, and the host-or-whole-URI fallback
  that three call sites each derived by hand now lives in one
  constructor. The `mixnet` module is declared in every build with its
  transport machinery feature-gated item-by-item, so the identity
  vocabulary is reachable from the always-compiled diary.
- BREAKING: the SOCKS5 endpoint is `std::net::SocketAddr` everywhere the
  wallet holds one — `MixnetStatus::socks5_addr`, `HostedTransport`,
  `MixnetProxy::attach`, `LightClient::mixnet_socks5_addr`, and the probe
  and sweep parameters. The address is parsed once where it enters: the
  spawned child's announcement line, `attach_mixnet`'s string parameter,
  and the typed host report; it renders back to a string only at the
  netutils dial calls. A spawned child announcing a non-parsing address
  now stays bootstrapping (refused by the readiness budget) instead of
  reaching Ready and failing at the route. The serialized `MixnetStatus`
  wire is unchanged: serde carries the address as the same string, and a
  malformed address on that wire refuses deserialization as suspicious.
- BREAKING: the mixnet route names the session slot's tunnel.
  `MixnetRoute::Mixnet` carries a `mixnet::SlotTunnel` instead of a bare
  address `String`. The tunnel refuses an address that does not parse as
  a socket address, stores the parsed `std::net::SocketAddr`, yields it
  once through `into_addr`, and lends it through `addr`; consumers render
  the dial string at their own seams. The zero-caller
  `MixnetRoute::socks5_proxy` accessor is removed.
- BREAKING: probing Correspondents while Mixnet Mode is deliberately
  switched off refuses with the new
  `LightClientError::ProbeRequiresMixnet`, which names the toggle-off,
  instead of mislabeling the state as `MixnetNotReady::Unattached`.
- BREAKING: a mobile platform that forbids subprocesses can now supply the mixnet
  transport. `mixnet::acquire` is public and adds the `ProxyHost` trait, which
  a host implements to answer a directory query and to start one proxy over
  a drawn Clutch, together with the `HostedTransport` record it answers
  with. `LightClient` gains `enable_mixnet_via_host`, the mobile twin of
  `enable_mixnet`. Both, and `start_mixnet_session`, now return
  `mixnet::acquire::TransportError` rather than `MixnetProxyError`, because an
  acquisition can fail before any proxy exists.
- BREAKING: the attach path carries the mobile platform host's bound Exit Node
  identities. `LightClient::attach_mixnet` takes an `exits: &[String]`
  parameter and `mixnet::ProvisionStrategy::Attach` gains an `exits` field;
  the attached transport's `Ready` publication reports them, so the
  session's exits-in-use draw is no longer vacuous on the attach path.
- BREAKING: the transport-acquisition path speaks the typed
  `mixnet::TransportError` instead of `String`. The enum names the
  missing acquirer, the discover mode's spawn and exit failures, the
  unseeded and exhausted Exit Pool, and the transport's bootstrap death
  (carrying the typed `NetOpFailure` detail), closed status channel, and
  missed readiness budget, and it wraps `MixnetProxyError`.
  `PriceError::TransportAcquisition` now carries it in place of `String`.
- BREAKING: the editorial layer moved from `wallet::summary::data` to
  `zingolib::perspective` (`ValueTransfer` and its kinds, the finsight
  rollups, and the `value_transfers` / `messages_containing` / `finsight` /
  `do_total_*` methods), behind the new `perspective` feature, which is off
  by default. A consumer that renders value transfers enables the feature
  and imports the types from `zingolib::perspective`.
- The `testutils` feature enables `perspective`, so the chain-generics
  value-transfer fixture is present whenever the test scaffolding is
  compiled.
- BREAKING: Health is implemented. `IndexerAttempt` gains a `phase` and an
  `exit`, so a failure is charged to the party the evidence names rather
  than to a category that cannot tell a tunnel failure from a server's.
  Every attempt updates an always-on, in-memory, session-scoped Health,
  whatever the diary's gates say; the feature-gated diary stays its
  opt-in export view, and its line format grows two columns while still
  loading six-column rows. The Correspondent draw consults Health as a
  binary eligibility filter with a floor, never as a weight, so the draw
  stays uniform and a partition cannot shrink the anonymity set.
- BREAKING: the session holds an Exit Pool, the sole issuer of Exit Node
  Reservations. Every acquisition draws a Clutch from it, recycles the
  unbound reservations the moment it binds one, and returns the
  Exclusive Lease when the transport's lifecycle ends, so two transports
  can never hold one exit. Every discovered node stays eligible for the
  whole session: population hygiene belongs to the upstream directory,
  not to an in-wallet statistic. The exclusion lists this replaces are
  gone. Reservations are owning values that recycle themselves when
  dropped, so every path — success, failure, or a hedged pull's
  cancellation — returns what it drew, and a session that cannot draw a
  ledgered Clutch refuses instead of letting the spawned binary select
  exits outside the ledger.
- BREAKING: every pull of a mixnet Transmission binds its own Exclusive
  exit (ADR 0039). A spawned session's send escalation consumes one
  Indexer Pool member per pull, acquiring inline past the complement,
  and tears each transport down when its pull ends, so an exit carries
  exactly one Correspondent contact. `TransmitRoute::Mixnet` now
  attests the winning pull's own tunnel rather than a shared one. An
  attached session shares the slot's tunnel as before. A pooled member
  whose transport died between take and use makes its pull refuse — and
  the price run refuse with `TransportError::DiedBeforeUse` — rather than
  silently degrading onto the slot's shared tunnel and mislabeling the
  diary's bound exit.
- BREAKING: the Correspondent Pools land. A spawned session keeps an
  Indexer Pool (two Exit-Bound transports) and a Price Source
  Pool (one Shared-exit transport), refilled in the background under
  `PrioritisePrivacy` and drained on disable. A drain bumps a generation
  and clears the acquirer, so a refill still in flight when the user
  disables Mixnet Mode stops its child and recycles its exit rather than
  admitting a live mixnet process into the drained pool. `update_current_price`
  on a spawned session consumes the price member — one fresh Shared
  exit per run, the refill draw excluding the spent exit — instead of
  riding the slot's shared tunnel; an attached session is unchanged.
  `PriceError` gains `TransportAcquisition`.
- BREAKING: `mixnet::correspondents` is absorbed into the new top-level
  `zingolib::correspondent` module, which compiles without the `nym`
  feature and adds the `Correspondable` trait — the party a Transmission
  addresses, implemented by the census `Indexer` and (under `nym`) by
  `PriceSource`, each yielding an https address and an accountable
  operator. The draw-eligibility functions stay `nym`-gated.
- BREAKING: there is no default server. `config::construct_indexer_uri`
  takes `String` instead of `Option<String>`, and the
  `DEFAULT_INDEXER_URI` / `DEFAULT_INDEXER_URI_TESTNET` re-exports are
  removed; an unpinned online session starts Indexerless and the
  Server-Selection Sweep selects its sync indexer.
- BREAKING: `LightClient::attach_mixnet` readiness is a data round trip
  through the mobile-platform-hosted endpoint to a census health indexer, retried
  once, because a listener that accepts TCP proves nothing about the mixnet
  carrying data; a data-dead endpoint lands `Died` rather than `Ready`. The
  loopback dial remains only as the cheap liveness watchdog after readiness.
- BREAKING: the send escalation is a hedged race (ADR 0040): a further Correspondent
  is contacted only after `TRANSMISSION_HEDGE_INTERVAL` of silence or a
  pull's failure, holding at most `RESERVATION_CLUTCH_SIZE` pulls in
  flight, replacing the serially gated one-two-three rounds. The
  six-Correspondent cap and the happy path's single-Correspondent
  discipline are unchanged.
- `config::ClientConfigBuilder`: `build` method now returns result for improved error handling.
- `config::construct_lightwalletd_uri`: `server` parameter changed from `Option<String>` to `String`. documentation
  updated to include options for defaults.
- BREAKING: the price fetch has no clearnet tier and compiles only with the `nym`
  feature. Without it `zingo-price` is types-only.
- BREAKING: `LightClient::enable_mixnet` takes a
  `R: zingo_netutils::responsiveness::Responsiveness` type parameter that names
  the acquisition's responsiveness class; `zingolib::nym` re-exports `Critical`,
  `NonCritical`, and `Responsiveness` for callers.
- BREAKING: the responsiveness classes are renamed for the tradeoff they
  declare: `zingolib::nym` re-exports `PrioritiseSpeed` (was `Critical`) and
  `PrioritisePrivacy` (was `NonCritical`). A class names the acquisition's
  declared priority, never who waits.
- BREAKING: the send-path vocabulary of ADRs 0036 and 0037 replaces "broadcast"
  and "witness" throughout the API. The config key `migration_broadcast_uri` is
  renamed `migration_transmission_uri` (builder:
  `set_migration_transmission_uri`). `wallet::migration` re-exports
  `TransmissionClient` and `PartTransmissionError` (were `BroadcastClient`,
  `BroadcastError`) and `TransmissionWindow` (was `BroadcastWindow`).
  `SplitStep::RoundBroadcast` is `SplitStep::RoundTransmitted`.
  `LightClientError` renames `MigrationBroadcastTargetIsSyncEndpoint` to
  `MigrationTransmissionTargetIsSyncEndpoint` and `NoEligibleBroadcastIndexer`
  to `NoEligibleCorrespondent`. `LightClient::probe_broadcast_indexers` is
  `probe_correspondents`, `broadcast_due_parts` is `transmit_due_parts`, and
  `auto_broadcast_if_due` is `auto_transmit_if_due`.
  `TransmitRoute::Mixnet`'s field `witness` is `correspondent`. The nym
  modules rename: `mixnet::broadcast` to `mixnet::correspondent_rotation` and
  `mixnet::broadcast_indexers` to `mixnet::correspondents`, with
  `CORRESPONDENT_INDEXERS` (was `BROADCAST_INDEXERS`). The migration modules
  `lightclient::migrate::{broadcast_grpc, broadcast_route}` rename to
  `{transmission_grpc, transmission_route}` with `GrpcTransmissionClient`,
  `RoutedTransmissionClient`, and `MixnetTransmissionClient`. The persisted
  part-state grammar (`PartState::Broadcast` and its stored strings) is
  deliberately unchanged: renaming a persisted token is a wallet-format event.
- Wallet file format is version 42. Versions 32 to 43 are read, 43 being a burned
  number carrying the final 42 layout (ADR 0015). An unreadable file falls back to
  a prefix-only salvage read so `recovery_info` still works.

### Removed
- **Breaking.** The indexer diary no longer touches disk. `IndexerHistoryHandle`
  keeps this session's attempts in memory and folds each into the session's
  Health, so `indexer-history.tsv` is never written and no record of which
  indexers this wallet contacted survives the process.
  `IndexerHistoryHandle::{beside_wallet, is_recording}` and
  `IndexerAttempt::exit` are gone with the file they served.
- **Breaking.** The `nym-diary` feature and `LightClient::set_indexer_diary`
  are gone. The feature gated a disk-backed handle that no longer exists, and
  the runtime opt-in gated writes that no longer happen.
- `mixnet::IP_CORRELATION_DISCLAIMER` - the frontend-facing disclaimer text. A
  library does not own the wording an application shows its user, so the text
  moved into `zingo-cli` as a private constant. A frontend that shows the
  IP-correlation risk now carries its own wording.
- The `mixnet` re-export of `zingo_netutils::responsiveness::{PrioritisePrivacy,
  PrioritiseSpeed, Responsiveness}` - the responsiveness partition retired with
  ADR 0044's single hedged acquisition policy, and no class reaches the API.
- `mixnet::sweep::indexer_lanes` and `mixnet::sweep::opening_wave_timed_out` -
  dead remnants of the wave-carried Sentinel. The Sentinel proof moved to the
  client's birth (ADR 0044), so the wave runs at its full width with no lane
  displaced, and a dead exit is condemned at birth rather than read off an
  all-timeout opening wave.
- `wallet::LightWallet::update_current_price` - the deprecated lock-holding
  price fetch. Its only callers were two tests, and the sequential
  `zingo_price` path it rode is itself removed; production fetches with
  `LightClient::update_current_price`, which races the sources outside the
  wallet lock and records the result under a briefly-held one.
- `wallet::summary::data::TransactionSummary::balance_delta` - the method had no
  callers and misreported a Zennies-donating self-send: `transaction_kind`
  exempts the donation address, so the `SendToSelf` arm reported only the fee
  while the wallet also moves the donation. A future consumer should derive
  balance deltas after the Zennies exemption moves to the viewmodel projection
  (#2612).
- `wallet::summary::data::TransactionSummaries::paid_fees` - its only caller was
  the `get_fees_paid_by_client` testutils helper, which now sums the fees itself
  (#2612).
- `wallet::summary::data::TransactionSummaries::txids` - called only by test
  code, which now inlines the one-line map (#2612).

## [5.0.0] - 2026-06-10

### Added
- `lightclient::LightClient::poll_sync_recovery()` — polls the sync task and,
  if it failed, returns `(SyncRecoveryObservables, String)` with the recommended
  recovery action and error description. Primary entry point for consumers
  (CLI, mobile, PC) to handle sync failures.
- impl TryFrom<&str> for `config::ChainType`
- `config::InvalidChainType`
- `lightclient::WalletMeta`: new public struct wrapping `LightWallet` with metadata and immutable wallet data
  stored outside the lock.
- `lightclient::LightClient`:
  - `chain_type` method: lock-free access to `ChainType`
  - `birthday` method: lock-free access to wallet birthday as `u32`
  - `mnemonic_phrase` method: lock-free access to the wallet's mnemonic phrase
  - `wallet_path` method returns wallet file path
  - `wallet_dir` method returns path to directory which holds wallet file
  - `wallet` method: returns `&Arc<RwLock<LightWallet>>`, replacing the former public field
  - `indexer: GrpcIndexer` field: owning the indexer connection directly
  - `backup_wallet_file` method to replace `ZingoConfig` method
  - updated `Debug` impl
- re-export `zingo_common_components::protocol::ActivationHeights` so test crates can unify zingo common types with
  zingolib lightclient construction
- `wallet::utils`: added `get_zcash_params_path` fn to replace `ZingoConfig` method.
- `config::WalletConfig` enum: replaces functionality of `wallet::WalletBase`. now encapsulates all wallet config for creation
  of a `wallet::Lightwallet` for each variant i.e. from seed or ufvk
- `testutils::default_test_wallet_settings`
- `wallet::WalletSettings`: `default` impl

### Changed
- Upgraded `zingo-netutils` from 3.0.0 to 5.0.1:
  - proto types now come from `lightwallet-protocol` via `zingo_netutils::lightwallet_protocol`.
  - `globally-public-transparent` feature gates are enabled.
- `lightclient::LightClient`:
  - `new` now installs the rustls ring crypto provider (idempotent) since
    `GrpcIndexer::new` pre-builds a TLS endpoint at construction time.
  - `indexer_uri` now returns `&http::Uri` instead of `Option<&http::Uri>`.
  - `set_indexer_uri` now returns `Result<(), zingo_netutils::GetClientError>` and
    constructs a new `GrpcIndexer` internally (`set_uri` was removed upstream).
  - `server_uri`: renamed `indexer_uri`
  - `set_server`: renamed `set_indexer_uri`
  - `pub wallet: Arc<RwLock<LightWallet>>` field is now private. replaced by `wallet` method.
  - `new` constructor: removed `chain_height` parameter which is now within the config
- `lightclient::error::LightClientError`: removed `TorClientError` variant.
- `config` module:
  - `ChainType`:
    - `Regtest` activation heights tuple variant field changed from zebra type to zingo common components type.
    - `fmt::Display` impl changed to give full network type names.
    - `zcash_protocol::consensus::Parameters` impl is no longer public to constrain external types in public API.
  - `ZingoConfig`:
    - renamed: `ClientConfig`
    - `wallet_settings` and `no_of_accounts` fields replaced by `wallet_config` field
    - `network_type` field renamed `chain_type`
    - reworked. public fields now private with public getter methods to constrain public API:
      - `wallet_dir` replaces `get_zingo_wallet_dir`
      - `chain_type` method replaces `chain` field
      - `indexer_uri` method replaces `lightwalletd_uri` field and `get_lightwalletd_uri` method
      - `build` renamed `builder`
      - `wallet_settings` and `no_of_accounts` methods replaced by `wallet_config` method
      - `get_zcash_params_path` replaced by `utils::get_zcash_params_path` fn
      - `backup_existing_wallet` replaced by `LightClient::backup_wallet_file`
  - `ClientConfigBuilder::build`: default `indexer_uri` is now `DEFAULT_INDEXER_URI`
    (`https://zec.rocks:443`) instead of an empty URI, since `GrpcIndexer::new`
    validates the scheme at construction.
  - `ZingoConfigBuilder`:
    - renamed: ClientConfigBuilder
    - reworked. public fields now private with public setter methods to constrain public API:
      - `create` renamed `build`
  - `DEFAULT_LIGHTWALLETD_SERVER` const: renamed `DEFAULT_INDEXER_URI`
  - `DEFAULT_TESTNET_LIGHTWALLETD_SERVER` const: renamed `DEFAULT_INDEXER_URI_TESTNET`
  - `DEVELOPER_DONATION_ADDRESS` const: moved to lib.rs
  - `ZENNIES_FOR_ZINGO_DONATION_ADDRESS` const: moved to lib.rs
  - `ZENNIES_FOR_ZINGO_TESTNET_ADDRESS` const: moved to lib.rs
  - `ZENNIES_FOR_ZINGO_REGTEST_ADDRESS` const: moved to lib.rs
  - `ZENNIES_FOR_ZINGO_AMOUNT` const: moved to lib.rs
  - `get_donation_address_for_chain` fn moved to lib.rs and renamed `get_zennies_for_zingo_address`
      now takes `ChainType` instead of `&ChainType`
  - `construct_lightwalletd_uri` fn: now returns result for handling URI errors
- `wallet::LightWallet`:
  - `pub network: ChainType` field is now private. Use `LightClient::chain_type()`.
  - `pub birthday: BlockHeight` field is now private. Use `LightClient::birthday()`.
  - `new` constructor:
    - `network` parameter renamed `chain_type`
    - `wallet_base`, `birthday` and `wallet_settings` fields replaced by `wallet_config` field
  - new wallet serialization version 41 due to changes to chain type fmt::Display. chain type is now encoded as u8 and output indexes changed to u32.
  - `update_current_price` method no longer takes `tor_client` parameter.
- `wallet::keys::unified::UnifiedKeyStore`:
  - `new_from_seed` method: `network` parameter renamed `chain_type` and now takes `ChainType` instead of `&ChainType`
  - `new_from_mnemonic` method: `network` parameter renamed `chain_type` and now takes `ChainType` instead of `&ChainType`
  - `new_from_ufvk` method: `network` parameter renamed `chain_type` and now takes `ChainType` instead of `&ChainType`
- `wallet::disk`:
  - serialized version incremented to 41 for serializing output indexes as u32 and chain types as u8 instead of string.
  - `read` module: `network` parameter renamed `chain_type`
- `wallet::error::WalletError`: added `WalletAlreadyCreated` variant
- `wallet::error::KeyError`: added `InvalidMnemonicPhrase` variant
- `wallet::summary::data`:
  - `NoteSummary`: `output_index` field is now u32.
  - `OutgoingNoteSummary`: `output_index` field is now u32.
  - `CoinSummary`: `output_index` field is now u32.
  - `OutgoingCoinSummary`: `output_index` field is now u32.
- `wallet::output::OutputRef`: `output_index` method now returns u32.

### Removed
- `regtest` feature: production binaries can now be tested in regtest mode.
- `config` module:
  - `DEFAULT_LOGFILE_NAME` constant
  - `ZingoConfig`:
    - `logfile_name` method
    - `get_log_config` method
    - `get_log_path` method
    - `create_testnet` method
    - `create_mainnet` method
    - `create_unconnected` method
  - `ZingoConfigBuilder`:
    - `set_logfile_name` method
  - `ChainFromStingError`: replaced by `InvalidChainType` error struct.
  - `chain_from_str`: replaced by impl TryFrom<&str> for `ChainType`
  - `ZingoConfig`:
    - `get_wallet_with_name_pathbuf`
    - `get_wallet_with_name_path`
    - `wallet_with_name_path_exists`
    - `get_wallet_pathbuf`
    - `wallet_exists(`
  - `DEFAULT_LOGFILE_NAME` constant.
  - `ZingoConfig`:
    - `logfile_name` field
    - `logfile_name()` method
    - `get_log_config()` method
    - `get_log_path()` method
  - `ZingoConfigBuilder::set_logfile_name()` method.
  - `load_clientconfig`: replaced by zingo config builder pattern (`ZingoConfigBuilder`)
- `wallet::LightWallet`: `mnemonic` method.
- `testutils::lightclient::new_client_from_save_buffer`
- `wallet::WalletBase`: no longer public. public functionality replaced by `config::WalletConfig`
- `lightclient::LightClient`:
  - `create_from_wallet` constructor: no longer needed as now covered by `new` due to config rework
  - `create_from_wallet_path` constructor: no longer needed as now covered by `new` due to config rework
  - `tor_client` method. Tor no longer supported. To be replaced by nym in coming release.
  - `create_tor_client` method.
  - `remove_tor_client` method.
- `testutils::build_fvk_client`

## [4.0.0] - 2026-06-05

### Changed
- `lightclient::error::LightClientError`: added `SyncLaunchErrror` variant.
- `data::Receiver`: From impl for Payment is now a TryFrom

## [3.0.1] - 2026-03-26

## [3.0.0] - 2026-03-02

### Added
- `lightclient::error::TransmissionError`: moved from `wallet::error` and simplified to much fewer variants more specific
to transmission.
- `wallet`: publicly re-exported `pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery, TransparentAddressDiscoveryScopes}`

### Changed
- `lightclient::LightClient::new`: no longer recommends the `chain_height` parameter to actually be {chain height - 100}. consumers should input the current chain height.
- `lightclient::error::LightClientError`:
  - `SyncError` fmt display altered
  - `SendError` variant added
  - `FileError` removed From impl for std::io::error
- `lightclient::error::SendError` - now includes all error types related to sending such as transmission and proposal errors.
- `wallet::LightWallet`:
  - removed `send_progress` field
  - `remove_unconfirmed_transactions` method renamed to `remove_failed_transactions` and now only removes transactions with the
new `Failed` status. Also now returns `wallet::error::WalletError`. No longer resets spends as spends are now reset when
a transaction is updated to `Failed` status. Transactions are automatically updated to `Failed` if transmission fails 4 times or
if the transaction expires before it is confirmed. Spends locked up in unconfirmed transactions for 3 blocks will also be reset
to release the funds, restoring balance and allowing funds to be spent in another transaction.
  - added `clear_proposal` method for removing an unconfirmed proposal from the wallet.
- `wallet::error::WalletError`:
  - added `ConversionFailed` variant
  - added `RemovalError` variant
  - added `TransactionNotFound` variant
  - added `TransactionRead` variant
  - added `BirthdayBelowSapling` variant
  - `TransactionWrite` removed From impl for std::io::error
  - `CalculateTxScanError` include fmt display of underlying error in fmt display
  - `ShardTreeError` fmt display altered
- `wallet::error::ProposeShieldError` - renamed `Insufficient` variant to `InsufficientFunds`
- `wallet::utils::interpret_memo_string`: changed name to `memo_bytes_from_string`. No longer decodes hex. Memo text will be displayed as inputted by the user.

### Removed
- `lightclient::LightClient::resend` - replaced by automatic retries due to issues with the current `resend` or `remove` user flow.
- `lightclient::LightClient::send_progress`
- `lightclient::error::QuickSendError`
- `lightclient::error::QuickShieldError`
- `lightclient::send_with_proposal` module - contents moved to `send` (parent) module.
- `wallet::send::SendProgress`
- `wallet::error::RemovalError` - variants added to `WalletError`
- `wallet::error::TransmissionError` - moved to `lightclient::error` module
- `error` module - unused

## [2.1.2] - 2026-01-14

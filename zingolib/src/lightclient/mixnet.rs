//! Mixnet Mode toggle (ADR 0011, consumption model A). Enabling spawns the
//! bundled `nym-proxy` child process. Disabling shuts it down. The mode
//! reflects the transport slot's state, and clearnet is reachable only by a
//! deliberate disable, never as a silent fallback: a session that never
//! enabled the mixnet, or whose enable failed, is `Unattached` and refuses.

use super::error::LightClientError;
use super::{LightClient, MixnetPriceFetch};

impl LightClient {
    /// Take whatever transport the slot holds and shut it down, leaving the
    /// `Unattached` that a failed enable also deliberately leaves behind,
    /// because the enable act revoked any standing clearnet consent and a
    /// failure must not silently reinstate a prior `SwitchedOff`.
    pub(super) async fn vacate_mixnet_slot(&mut self) {
        self.correspondent_pools.drain_all().await;
        if let crate::nym::MixnetSlot::Attached(running) =
            std::mem::replace(&mut self.mixnet_slot, crate::nym::MixnetSlot::Unattached)
        {
            running.stop().await;
        }
        // Dropping the slot's Clutch recycles the session tunnel's
        // reservations after the transport is gone.
        self.slot_clutch.clear();
    }

    /// Enable Mixnet Mode by spawning the bundled `nym-proxy` binary at
    /// `binary_path`, returning immediately while [`Self::mixnet_mode`]
    /// reports `Bootstrapping` until the proxy announces its SOCKS5 address
    /// and becomes `Ready`, replacing any already-running proxy, and leaving
    /// a spawn failure `Unattached` — refusing the mixnet surfaces, never
    /// falling back to clearnet.
    pub async fn enable_mixnet<R: zingo_netutils::responsiveness::Responsiveness>(
        &mut self,
        binary_path: &std::path::Path,
    ) -> Result<(), crate::nym::acquire::TransportError> {
        self.enable_mixnet_from(
            std::sync::Arc::new(crate::nym::acquire::SpawnedBinary::at(
                binary_path.to_path_buf(),
            )),
            R::CLASS,
        )
        .await
    }

    /// Enables Mixnet Mode on a platform that forbids subprocesses, taking
    /// every transport from `host` instead of spawning one.
    pub async fn enable_mixnet_via_host<R: zingo_netutils::responsiveness::Responsiveness>(
        &mut self,
        host: std::sync::Arc<dyn crate::nym::acquire::ProxyHost>,
    ) -> Result<(), crate::nym::acquire::TransportError> {
        self.enable_mixnet_from(
            std::sync::Arc::new(crate::nym::acquire::HostedProxy::owned_by(host)),
            R::CLASS,
        )
        .await
    }

    /// Enables Mixnet Mode over `acquirer`, the one seam both platforms fill.
    async fn enable_mixnet_from(
        &mut self,
        acquirer: std::sync::Arc<dyn crate::nym::acquire::TransportAcquirable>,
        class: zingo_netutils::responsiveness::ResponsivenessClass,
    ) -> Result<(), crate::nym::acquire::TransportError> {
        self.vacate_mixnet_slot().await;
        let clutch = match self
            .correspondent_pools
            .draw_clutch(acquirer.as_ref())
            .await
        {
            Ok(clutch) => clutch,
            Err(refusal) => {
                // A session that cannot draw a ledgered Clutch refuses;
                // the spawned binary must never self-draw outside the
                // reservation ledger.
                self.publish_mixnet_slot_state();
                return Err(refusal);
            }
        };
        let nodes = crate::correspondent::pool::exit_pool::clutch_nodes(&clutch);
        match crate::nym::acquire::TransportAcquirable::acquire(
            acquirer.as_ref(),
            class,
            &nodes,
            std::sync::Arc::clone(&self.mixnet_status),
        )
        .await
        {
            Ok(proxy) => {
                // The spawn already published Bootstrapping into the session
                // channel; nothing further to announce here. The Clutch is
                // held for the tunnel's life and recycled on vacate.
                self.mixnet_slot = crate::nym::MixnetSlot::Attached(proxy);
                self.slot_clutch = clutch;
                self.correspondent_pools.set_acquirer(acquirer);
                self.correspondent_pools.ensure_filled();
                Ok(())
            }
            Err(error) => {
                // A failed enable leaves Unattached (the user's enable revoked
                // any standing clearnet consent); subscribers must see it.
                // The drawn Clutch drops here, recycling its reservations.
                self.publish_mixnet_slot_state();
                Err(error)
            }
        }
    }

    /// Attaches Mixnet Mode to an already-running, platform-hosted SOCKS5
    /// endpoint that bound `exits`, replacing any running transport.
    pub async fn attach_mixnet(
        &mut self,
        socks5_addr: &str,
        exits: &[crate::nym::ExitNodeId],
    ) -> Result<(), crate::nym::MixnetProxyError> {
        let socks5_addr: std::net::SocketAddr =
            socks5_addr
                .parse()
                .map_err(|_| crate::nym::MixnetProxyError::InvalidAddress {
                    addr: socks5_addr.to_string(),
                })?;
        self.vacate_mixnet_slot().await;
        match crate::nym::MixnetProxy::attach(
            socks5_addr,
            exits,
            std::sync::Arc::clone(&self.mixnet_status),
        ) {
            Ok(proxy) => {
                self.mixnet_slot = crate::nym::MixnetSlot::Attached(proxy);
                Ok(())
            }
            Err(error) => {
                self.publish_mixnet_slot_state();
                Err(error)
            }
        }
    }

    /// Disable Mixnet Mode — the deliberate, per-session choice that alone
    /// reaches [`MixnetMode::SwitchedOff`](crate::nym::MixnetMode) — shutting
    /// down any running transport so the mixnet-only surfaces route over
    /// clearnet as informed consent.
    pub async fn disable_mixnet(&mut self) {
        self.vacate_mixnet_slot().await;
        self.mixnet_slot = crate::nym::MixnetSlot::SwitchedOff;
        self.publish_mixnet_slot_state();
    }

    /// Publish the slot's current state into the session status channel,
    /// called only after a slot transition settles — never mid-replacement —
    /// so subscribers see deliberate states and not the transient unattached
    /// between a vacate and its successor.
    pub(super) fn publish_mixnet_slot_state(&self) {
        self.mixnet_status.send_replace(crate::nym::MixnetStatus {
            mode: self.mixnet_slot.mode(),
            // None for the true slot states; the pinned address of a test
            // stand-in, whose Ready must not publish addressless.
            socks5_addr: self.mixnet_slot.socks5_addr(),
            exits: self.mixnet_slot.exits(),
            bootstrap_detail: None,
            death: None,
        });
    }

    /// The driver entry a session calls at its go-online moment, which under
    /// [`MixnetStartPolicy::ForcedOn`](crate::nym::MixnetStartPolicy)
    /// provisions the transport by `strategy` (the bundled binary spawned
    /// from the consumer's platform hints, or an attach to a platform-hosted
    /// endpoint) so the bootstrap overlaps sync, under
    /// [`MixnetStartPolicy::OptedOutThisSession`](crate::nym::MixnetStartPolicy)
    /// records the startup opt-out as the explicit act that reaches switched
    /// off, returns any provisioning failure typed while leaving the mode
    /// unattached — refusal, never a silent clearnet — and never respawns on
    /// its own, recovery staying explicit through
    /// [`MixnetMode::needs_recovery`](crate::nym::MixnetMode::needs_recovery).
    pub async fn start_mixnet_session(
        &mut self,
        strategy: crate::nym::ProvisionStrategy<'_>,
        policy: crate::nym::MixnetStartPolicy,
    ) -> Result<(), crate::nym::acquire::TransportError> {
        match policy {
            crate::nym::MixnetStartPolicy::OptedOutThisSession => {
                self.disable_mixnet().await;
                Ok(())
            }
            crate::nym::MixnetStartPolicy::ForcedOn => match strategy {
                crate::nym::ProvisionStrategy::Spawn(hints) => {
                    let path = crate::nym::provision::resolve_proxy_path(&hints);
                    log::info!("mixnet session start: spawning nym-proxy at {path}");
                    // The go-online moment is a user act: someone is waiting.
                    self.enable_mixnet::<zingo_netutils::responsiveness::PrioritiseSpeed>(
                        std::path::Path::new(&path),
                    )
                    .await
                }
                crate::nym::ProvisionStrategy::Attach { socks5_addr, exits } => self
                    .attach_mixnet(socks5_addr, exits)
                    .await
                    .map_err(crate::nym::acquire::TransportError::from),
            },
        }
    }

    /// Subscribe to Mixnet Mode — the receiving half of the session's one
    /// status channel — whose keep-only-latest push delivers a typed
    /// [`MixnetStatus`](crate::nym::MixnetStatus) snapshot on every
    /// transition, in a receiver independent of this client borrow that
    /// survives enable/disable cycles.
    pub fn subscribe_mixnet_status(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::nym::MixnetStatus> {
        self.mixnet_status.subscribe()
    }

    /// The current Mixnet Mode, read from the transport slot:
    /// [`MixnetMode::Unattached`](crate::nym::MixnetMode) before any enable
    /// (and after a failed one),
    /// [`MixnetMode::SwitchedOff`](crate::nym::MixnetMode) after the
    /// deliberate disable, otherwise the transport's lifecycle state
    /// (bootstrapping, ready, or died).
    pub fn mixnet_mode(&self) -> crate::nym::MixnetMode {
        self.mixnet_slot.mode()
    }

    /// The local SOCKS5 address while Mixnet Mode is ready.
    pub fn mixnet_socks5_addr(&self) -> Option<std::net::SocketAddr> {
        self.mixnet_slot.socks5_addr()
    }

    /// Switch Mixnet Mode on for a chain-mock test, with the slot reporting
    /// [`MixnetMode::Ready`](crate::nym::MixnetMode) at `socks5_addr` while
    /// no child, watcher, or probe stands behind it, so the test walks the
    /// same fail-closed route resolver and escalation orchestration a live
    /// Ready session does and the transmit path submits over the mock
    /// indexer's channel without ever dialing the address.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn switch_on_mixnet_for_tests(&mut self, socks5_addr: &str) {
        self.vacate_mixnet_slot().await;
        self.mixnet_slot = crate::nym::MixnetSlot::AttachedForTests {
            socks5_addr: socks5_addr
                .parse()
                .expect("the test stand-in socks5 address parses"),
        };
        // Every slot transition publishes (the one-shared-watch invariant),
        // the stand-in included.
        self.publish_mixnet_slot_state();
    }

    /// The proxy's latest bootstrap progress line while Mixnet Mode is
    /// bootstrapping, so a user interface can narrate the connect race.
    pub fn mixnet_bootstrap_detail(&self) -> Option<String> {
        self.mixnet_slot
            .proxy()
            .and_then(|proxy| proxy.bootstrap_detail())
    }

    /// Why the transport died, while Mixnet Mode is
    /// [`MixnetMode::Died`](crate::nym::MixnetMode) and the watcher held a
    /// typed cause: the [`zingo_net_diag::NetOpFailure`] record naming the
    /// stage, the target, and the cause chain as a vector, so a `died`
    /// verdict carries *why* without anyone parsing prose.
    pub fn mixnet_death_detail(&self) -> Option<zingo_net_diag::NetOpFailure> {
        self.mixnet_slot
            .proxy()
            .and_then(|proxy| proxy.death_detail())
    }

    /// The latched death read whole — its moment and, when the watcher held
    /// one, its typed cause — while Mixnet Mode is
    /// [`MixnetMode::Died`](crate::nym::MixnetMode) and `None` in every
    /// other mode, the moment distinguishing a stale latch from a fresh one
    /// through [`crate::nym::DeathReport::age`].
    pub fn mixnet_death_report(&self) -> Option<crate::nym::DeathReport> {
        self.mixnet_slot
            .proxy()
            .and_then(|proxy| proxy.death_report())
    }

    /// Resolve the fail-closed route every mixnet-only surface must obey —
    /// the mixnet proxy when
    /// [`MixnetMode::Ready`](crate::nym::MixnetMode::Ready), clearnet only
    /// when switched off (the deliberate toggle-off), and a refusal while
    /// unattached, bootstrapping, or died — as the single resolver that
    /// send, price-fetch, and the liveness probe share.
    pub fn mixnet_route(&self) -> Result<crate::nym::MixnetRoute, crate::nym::MixnetNotReady> {
        crate::nym::resolve_route(self.mixnet_mode(), self.mixnet_socks5_addr())
    }

    /// Runs the mixnet liveness probe — concurrent `GetLightdInfo` calls
    /// through the session's SOCKS5 proxy, each appending its outcome to the
    /// cross-session indexer history — against `target`, or against every
    /// Correspondent when `target` is `None`, with no clearnet leg and a
    /// refusal while the mixnet transport is not ready.
    pub async fn probe_correspondents(
        &self,
        target: Option<http::Uri>,
        timeout: std::time::Duration,
    ) -> Result<Vec<crate::nym::probe::MixnetProbe>, crate::lightclient::error::LightClientError>
    {
        let socks5_addr = match self.mixnet_route()? {
            crate::nym::MixnetRoute::Mixnet(tunnel) => tunnel.into_addr(),
            crate::nym::MixnetRoute::Clearnet => {
                return Err(crate::lightclient::error::LightClientError::ProbeRequiresMixnet);
            }
        };
        if let Some(uri) = &target
            && !crate::nym::probe::probe_eligible(uri)
        {
            return Err(
                crate::lightclient::error::LightClientError::IneligibleProbeTarget(uri.clone()),
            );
        }
        let targets: Vec<http::Uri> = target
            .map_or_else(crate::correspondent::correspondent_indexers, |uri| {
                vec![uri]
            })
            .into_iter()
            .filter(crate::nym::probe::probe_eligible)
            .collect();
        let history = self.indexer_history.clone();
        Ok(futures::future::join_all(targets.iter().map(|indexer| {
            crate::nym::probe::probe_indexer(indexer, socks5_addr, timeout, &history)
        }))
        .await)
    }

    /// Update and return the current ZEC price in USD by racing the three
    /// price sources (Gemini, Kraken, CoinGecko) through the mixnet tunnel —
    /// hiding the client IP, taking the first answer, erroring only when
    /// every source fails and then naming each source's typed failure, and
    /// returning the tunnel endpoint, the winning source, and the round-trip
    /// time as per-fetch route evidence — while failing closed in every
    /// other state: a typed [`MixnetNotReady`](crate::nym::MixnetNotReady)
    /// refusal while unattached, bootstrapping, or died, and
    /// [`LightClientError::PriceFetchRequiresMixnet`] while switched off,
    /// because the switched-off consent covers Transmission and never a
    /// third-party price API outside the Zcash ecosystem.
    pub async fn update_current_price(&self) -> Result<MixnetPriceFetch, LightClientError> {
        let socks5_addr = match self.mixnet_route()? {
            crate::nym::MixnetRoute::Mixnet(tunnel) => tunnel.into_addr(),
            crate::nym::MixnetRoute::Clearnet => {
                return Err(LightClientError::PriceFetchRequiresMixnet);
            }
        };

        // A spawned session runs the race over its own Price Source Pool
        // member — one fresh Shared exit per run, never the slot's shared
        // tunnel — while an attached session's single platform endpoint
        // carries it as before. The consumed member is stopped whatever
        // the outcome, and the refill draw excludes its exit.
        let pooled = if self
            .mixnet_slot
            .proxy()
            .is_some_and(crate::nym::MixnetProxy::is_spawned)
            && self.correspondent_pools.acquirer().is_some()
        {
            Some(
                self.correspondent_pools
                    .take_or_acquire(|pools| &pools.price)
                    .await
                    .map_err(crate::wallet::error::PriceError::TransportAcquisition)?,
            )
        } else {
            None
        };
        // A pooled member carries its own fresh Shared exit; only an attached
        // session rides the slot's shared tunnel. A member whose transport
        // reports no address died between take and use, so the run refuses
        // rather than silently degrading to the slot tunnel.
        let via_socks5 = match pooled.as_ref() {
            Some(member) => match member.addr() {
                Some(addr) => addr,
                None => {
                    if let Some(member) = pooled {
                        member.retire().await;
                        self.correspondent_pools.ensure_filled();
                    }
                    return Err(LightClientError::from(
                        crate::wallet::error::PriceError::TransportAcquisition(
                            crate::nym::acquire::TransportError::DiedBeforeUse,
                        ),
                    ));
                }
            },
            None => socks5_addr,
        };

        // The fetch runs outside the wallet lock (the net-diag
        // polling-blackout remedy), so a hung tunnel can no longer freeze
        // every wallet-state observer. All sources race through the one
        // tunnel at full width; the first answer wins and the losing legs
        // are cancelled.
        let dispatched = std::time::Instant::now();
        let raced = zingo_price::race_current_price(Some(&via_socks5.to_string())).await;
        if let Some(member) = pooled {
            member.retire().await;
            self.correspondent_pools.ensure_filled();
        }
        let raced = raced.map_err(crate::wallet::error::PriceError::from)?;
        let round_trip = dispatched.elapsed();
        self.wallet().write().await.record_price_update(raced.price);
        Ok(MixnetPriceFetch {
            usd: raced.price.price_usd,
            source: raced.source,
            round_trip,
            via_socks5: via_socks5.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {

    mod price_fetch_contract {
        //! The price-fetch error contract for the mixnet route (ADR 0011,
        //! amendments 2026-07-23 and 2026-07-27).
        //!
        //! Every way the opt-in mixnet price fetch can fail must arrive at
        //! the API surface as a typed [`LightClientError`] variant with its
        //! source chain intact: never prose in the data channel, never a
        //! silent clearnet fallback. The route pre-flight variants
        //! (`PriceFetchRequiresMixnet`, `MixnetNotReady::{Unattached,
        //! Bootstrapping, Died}`) pair with `nym::route`'s own
        //! `resolve_route` tests; the tests here pin the surface wiring. The
        //! transport-leg contract (typed connect and timeout failures with
        //! their cause chains) is pinned in `zingo-price`'s own tests,
        //! beside the mechanism.
        use crate::lightclient::LightClient;
        use crate::lightclient::error::LightClientError;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        fn wallet() -> crate::wallet::LightWallet {
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build()
        }

        /// A never-enabled mixnet is a typed refusal, never a clearnet
        /// fallback: the fresh client is `Unattached` (absence is not
        /// consent, ADR 0011 amendment 2026-07-28), and the route pre-flight
        /// runs before any network object is built, so no packet leaves the
        /// process.
        #[tokio::test]
        async fn an_unattached_mixnet_is_a_typed_refusal() {
            let client = LightClient::new_for_test(wallet()).await;

            let error = client
                .update_current_price()
                .await
                .expect_err("no transport was ever enabled, so the mixnet fetch must refuse");
            assert!(
                matches!(
                    error,
                    LightClientError::MixnetNotReady(crate::nym::MixnetNotReady::Unattached)
                ),
                "the refusal must be typed, not prose: {error}"
            );
        }

        /// Mixnet Mode switched off is equally a typed refusal for the
        /// opt-in mixnet fetch: the caller demanded the private route, so a
        /// consented-clearnet mode answers `PriceFetchRequiresMixnet` rather
        /// than quietly fetching over clearnet.
        #[tokio::test]
        async fn switched_off_mode_is_a_typed_refusal() {
            let mut client = LightClient::new_for_test(wallet()).await;
            client.disable_mixnet().await;

            let error = client
                .update_current_price()
                .await
                .expect_err("switched off consents to clearnet, not to a mixnet fetch");
            assert!(
                matches!(error, LightClientError::PriceFetchRequiresMixnet),
                "the refusal must be typed, not prose: {error}"
            );
        }

        /// The startup opt-out is the explicit act (ADR 0024, consent at
        /// start): a deliberate disable on a fresh, never-enabled client
        /// lands SwitchedOff — not Unattached — and the route resolver
        /// consents to clearnet. This is the transition zingo-cli's
        /// --no-mixnet flag records at session start.
        #[tokio::test]
        async fn disable_before_any_enable_records_clearnet_consent() {
            let mut client = LightClient::new_for_test(wallet()).await;
            assert_eq!(client.mixnet_mode(), crate::nym::MixnetMode::Unattached);

            client.disable_mixnet().await;

            assert_eq!(client.mixnet_mode(), crate::nym::MixnetMode::SwitchedOff);
            assert!(matches!(
                client.mixnet_route(),
                Ok(crate::nym::MixnetRoute::Clearnet)
            ));
        }
    }

    mod session_driver_contract {
        //! The session driver's contract (ADR 0024, decision 2).
        //!
        //! The driver entry is the one call a session makes at its go-online
        //! moment; these tests pin its consent-at-start semantics, its typed
        //! refusal on a failed provisioning, the push delivery of every
        //! transition through the session's one status channel, and the
        //! Died-only recovery predicate.
        use crate::lightclient::LightClient;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        fn wallet() -> crate::wallet::LightWallet {
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build()
        }

        /// The driver entry honors the startup opt-out (ADR 0024, consent
        /// at start): OptedOutThisSession lands SwitchedOff without
        /// provisioning anything — the strategy is never exercised — and
        /// the transition reaches subscribers through the session channel.
        #[tokio::test]
        async fn the_driver_records_the_startup_opt_out_and_publishes_it() {
            let mut client = LightClient::new_for_test(wallet()).await;
            let subscriber = client.subscribe_mixnet_status();
            assert_eq!(
                subscriber.borrow().mode,
                crate::nym::MixnetMode::Unattached,
                "the channel opens in the ground state"
            );

            client
                .start_mixnet_session(
                    // A hint set that resolves to no real binary: the
                    // opt-out branch must never try to spawn it.
                    crate::nym::ProvisionStrategy::Spawn(
                        crate::nym::provision::SpawnHints::default(),
                    ),
                    crate::nym::MixnetStartPolicy::OptedOutThisSession,
                )
                .await
                .expect("the opt-out provisions nothing and cannot fail");

            assert_eq!(client.mixnet_mode(), crate::nym::MixnetMode::SwitchedOff);
            assert_eq!(
                subscriber.borrow().mode,
                crate::nym::MixnetMode::SwitchedOff,
                "the slot transition must reach subscribers"
            );
        }

        /// A forced-on attach to a malformed address fails typed and leaves
        /// Unattached — refusal, never clearnet — and publishes the settled
        /// state so a subscriber cannot be left staring at a stale mode.
        #[tokio::test]
        async fn a_failed_forced_on_start_publishes_unattached() {
            let mut client = LightClient::new_for_test(wallet()).await;
            let subscriber = client.subscribe_mixnet_status();

            let error = client
                .start_mixnet_session(
                    crate::nym::ProvisionStrategy::Attach {
                        socks5_addr: "not-a-socket-address",
                        exits: &[],
                    },
                    crate::nym::MixnetStartPolicy::ForcedOn,
                )
                .await
                .expect_err("a malformed attach address must refuse");
            assert!(matches!(
                error,
                crate::nym::acquire::TransportError::Proxy(
                    crate::nym::MixnetProxyError::InvalidAddress { .. }
                )
            ));

            assert_eq!(client.mixnet_mode(), crate::nym::MixnetMode::Unattached);
            assert_eq!(
                subscriber.borrow().mode,
                crate::nym::MixnetMode::Unattached,
                "the failed start's settled state must reach subscribers"
            );
        }

        /// The attached transport's lifecycle reaches subscribers end to
        /// end: a forced-on attach to a refusing localhost port publishes
        /// bootstrapping, then died with the typed readiness failure — all
        /// pushed, never polled. A deliberate disable afterwards publishes
        /// SwitchedOff and, because stop() awaits the aborted watcher, no
        /// stale death can be published over it.
        #[tokio::test]
        async fn attach_lifecycle_and_disable_reach_subscribers_in_order() {
            let mut client = LightClient::new_for_test(wallet()).await;
            let mut subscriber = client.subscribe_mixnet_status();

            client
                .start_mixnet_session(
                    // Port 9 (discard) refuses: readiness fails fast and
                    // the driver lands Died.
                    crate::nym::ProvisionStrategy::Attach {
                        socks5_addr: "127.0.0.1:9",
                        exits: &[],
                    },
                    crate::nym::MixnetStartPolicy::ForcedOn,
                )
                .await
                .expect("a well-formed address attaches");

            let died = subscriber
                .wait_for(|status| status.mode == crate::nym::MixnetMode::Died)
                .await
                .expect("the publisher outlives the wait")
                .clone();
            assert!(
                died.death.and_then(|report| report.detail).is_some(),
                "an attach readiness failure must publish its typed cause"
            );

            client.disable_mixnet().await;
            assert_eq!(
                subscriber.borrow_and_update().mode,
                crate::nym::MixnetMode::SwitchedOff
            );
            tokio::task::yield_now().await;
            assert!(
                !subscriber.has_changed().expect("the publisher is alive"),
                "no stale transport publication may follow the deliberate disable"
            );
        }

        /// The recovery predicate is Died only: the ground state carries no
        /// online intent (a wallet may never have consented to
        /// connectivity), switched off is consent revocation's territory,
        /// and the live states need no repair. Exhaustive over ALL so a new
        /// state must take a position.
        #[test]
        fn the_recovery_predicate_is_died_only() {
            for mode in crate::nym::MixnetMode::ALL {
                assert_eq!(
                    mode.needs_recovery(),
                    matches!(mode, crate::nym::MixnetMode::Died),
                    "{mode} must {}need recovery",
                    if matches!(mode, crate::nym::MixnetMode::Died) {
                        ""
                    } else {
                        "not "
                    }
                );
            }
        }
    }
}

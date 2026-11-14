mod price_fetch_contract {
    //! The price-fetch error contract: every failure reaches the API
    //! as a typed [`LightClientError`], and a nakednet leg runs only
    //! with consent. Route refusals pair with `mixnet::route`'s tests;
    //! transport-leg failures are pinned in the `price` module.
    use crate::lightclient::LightClient;
    use crate::lightclient::error::LightClientError;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

    fn wallet() -> crate::wallet::LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build()
    }

    /// A never-enabled mixnet is a typed refusal, never a nakednet
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
                LightClientError::MixnetNotReady(crate::mixnet::MixnetNotReady::Unattached)
            ),
            "the refusal must be typed, not prose: {error}"
        );
    }

    /// The price fetch is mixnet-only (ADR 0011, amendment 2026-09-11):
    /// a switched-off transport refuses it as the unattached one does,
    /// with no nakednet leg.
    #[tokio::test]
    async fn switched_off_mode_refuses_the_price_fetch() {
        let mut client = LightClient::new_for_test(wallet()).await;
        client.disable_mixnet().await;

        let error = client
            .update_current_price()
            .await
            .expect_err("a switched-off transport must refuse the price fetch");
        assert!(
            matches!(
                error,
                LightClientError::MixnetNotReady(crate::mixnet::MixnetNotReady::Unattached)
            ),
            "the refusal must be typed, not prose: {error}"
        );
    }

    /// The transmit policy is a send-only input, so the
    /// nakednet policy never opens a nakednet leg for the price fetch.
    /// Falsified if the fetch under the nakednet policy does anything
    /// but refuse on an unattached transport.
    #[tokio::test]
    async fn the_nakednet_policy_never_reaches_the_price_fetch() {
        let client = LightClient::new_for_test(wallet()).await;
        client.set_transmit_policy(crate::mixnet::TransmitPolicy::Nakednet);

        let error = client
            .update_current_price()
            .await
            .expect_err("the transmit policy must not consent to a nakednet price fetch");
        assert!(
            matches!(
                error,
                LightClientError::MixnetNotReady(crate::mixnet::MixnetNotReady::Unattached)
            ),
            "the refusal must be typed, not prose: {error}"
        );
    }

    /// A deliberate disable on a fresh, never-enabled client lands
    /// SwitchedOff rather than Unattached, and both resolvers read it
    /// as a missing transport.
    #[tokio::test]
    async fn disable_before_any_enable_lands_switched_off() {
        let mut client = LightClient::new_for_test(wallet()).await;
        assert_eq!(
            client.read_mixnet_indicator(),
            crate::mixnet::Indicator::Unattached
        );

        client.disable_mixnet().await;

        assert_eq!(
            client.read_mixnet_indicator(),
            crate::mixnet::Indicator::SwitchedOff
        );
        assert_eq!(
            client.mixnet_only_route().err(),
            Some(crate::mixnet::MixnetNotReady::Unattached)
        );
        assert_eq!(
            client.send_route().err(),
            Some(crate::mixnet::MixnetNotReady::Unattached)
        );
    }
}

mod transmit_policy {
    //! The session's transmit policy (ADR 0011, amendment 2026-09-11):
    //! the consumer's per-session choice of where a transaction
    //! travels, readable, flippable through `&self`, and reaching the
    //! send route alone. The state by policy matrix is pinned in
    //! `mixnet::route`; these pin the client's wiring of it.
    use crate::lightclient::LightClient;
    use crate::mixnet::{MixnetNotReady, MixnetRoute, TransmitPolicy};
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

    fn wallet() -> crate::wallet::LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build()
    }

    #[tokio::test]
    async fn every_session_starts_under_the_mixnet_policy() {
        let client = LightClient::new_for_test(wallet()).await;
        assert_eq!(client.transmit_policy(), TransmitPolicy::Mixnet);
    }

    #[tokio::test]
    async fn the_policy_flips_while_the_client_runs_and_reads_back() {
        let client = LightClient::new_for_test(wallet()).await;
        client.set_transmit_policy(TransmitPolicy::Nakednet);
        assert_eq!(client.transmit_policy(), TransmitPolicy::Nakednet);
        client.set_transmit_policy(TransmitPolicy::Mixnet);
        assert_eq!(client.transmit_policy(), TransmitPolicy::Mixnet);
    }

    /// The nakednet policy routes a send at once, on a client whose
    /// transport was never enabled.
    #[tokio::test]
    async fn the_nakednet_policy_routes_a_send_over_an_unattached_transport() {
        let client = LightClient::new_for_test(wallet()).await;
        assert_eq!(client.send_route(), Err(MixnetNotReady::Unattached));

        client.set_transmit_policy(TransmitPolicy::Nakednet);

        assert_eq!(client.send_route(), Ok(MixnetRoute::Nakednet));
    }

    /// A flip back to the mixnet policy restores the refusal, so the
    /// policy is read at resolution time and never latched.
    #[tokio::test]
    async fn flipping_back_to_the_mixnet_policy_restores_the_refusal() {
        let client = LightClient::new_for_test(wallet()).await;
        client.set_transmit_policy(TransmitPolicy::Nakednet);
        assert_eq!(client.send_route(), Ok(MixnetRoute::Nakednet));

        client.set_transmit_policy(TransmitPolicy::Mixnet);

        assert_eq!(client.send_route(), Err(MixnetNotReady::Unattached));
    }
}

mod consent_holds {
    //! Revoked consent stops every networking act the session started
    //! (findings 2 and 9 of the 2026-08-14 PR #2695 review).
    use crate::mixnet::ExitNodeId;

    /// A Standing Client over an attach to `addr`, for slot tests.
    fn attached_client(addr: std::net::SocketAddr) -> crate::mixnet::StandingClient {
        let proxy = crate::mixnet::MixnetProxy::attach(
            addr,
            &[ExitNodeId::from("exit-alpha")],
            crate::mixnet::test_indexer(),
            crate::mixnet::status_publisher(),
        )
        .expect("the attach accepts a bound exit");
        crate::mixnet::StandingClient::new(proxy, None, true)
    }

    /// HYPOTHESIS: a failover's replacement installs only while the slot
    /// still holds the condemned client whose enable it continues, so a
    /// consent revoked mid-birth is never reversed. Falsified if the
    /// install overwrites a SwitchedOff slot.
    #[tokio::test]
    async fn a_switched_off_slot_refuses_the_failover_client() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let slot = std::sync::Arc::new(std::sync::Mutex::new(
            crate::mixnet::MixnetSlot::SwitchedOff,
        ));

        let Err(refused) = super::super::install_failover_client(&slot, attached_client(addr))
        else {
            panic!("a switched-off slot must refuse the continuation");
        };
        refused.stop().await;

        assert!(
            matches!(
                &*slot.lock().expect("mixnet slot mutex"),
                crate::mixnet::MixnetSlot::SwitchedOff
            ),
            "the revoked consent stands"
        );
    }

    /// HYPOTHESIS: the install succeeds exactly when the slot holds the
    /// condemned client the failover replaces, yielding the superseded
    /// slot for teardown. Falsified if a condemned slot refuses.
    #[tokio::test]
    async fn a_condemned_slot_accepts_the_failover_client() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let old = attached_client(addr);
        old.condemn();
        let slot = std::sync::Arc::new(std::sync::Mutex::new(crate::mixnet::MixnetSlot::Attached(
            old,
        )));

        let Ok(superseded) = super::super::install_failover_client(&slot, attached_client(addr))
        else {
            panic!("the condemned slot must accept its continuation");
        };
        if let crate::mixnet::MixnetSlot::Attached(old) = superseded {
            old.stop().await;
        }

        assert!(
            matches!(
                &*slot.lock().expect("mixnet slot mutex"),
                crate::mixnet::MixnetSlot::Attached(client) if !client.is_condemned()
            ),
            "the replacement stands in the slot"
        );
    }

    /// HYPOTHESIS: a rotation installs over the healthy incumbent it set
    /// out to replace, which is the one client no other path can
    /// replace, and yields it for retirement. Falsified if a healthy
    /// slot refuses the hand-off.
    #[tokio::test]
    async fn a_healthy_slot_accepts_the_rotated_client() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let slot = std::sync::Arc::new(std::sync::Mutex::new(crate::mixnet::MixnetSlot::Attached(
            attached_client(addr),
        )));

        let Ok(superseded) = super::super::install_rotated_client(&slot, attached_client(addr))
        else {
            panic!("a healthy slot must accept its rotation");
        };
        let crate::mixnet::MixnetSlot::Attached(outgoing) = superseded else {
            panic!("the rotation yields the incumbent it replaced");
        };
        outgoing.retire().await;

        assert!(
            matches!(
                &*slot.lock().expect("mixnet slot mutex"),
                crate::mixnet::MixnetSlot::Attached(client) if !client.is_condemned()
            ),
            "the replacement stands in the slot"
        );
    }

    /// HYPOTHESIS: rotation and failover are exact complements over the
    /// slot, so a client is replaceable by one path and never by both.
    /// Falsified if a condemned client accepts a rotation or a healthy
    /// one accepts a failover, either of which lets a rotation and a
    /// failover both install over the same incumbent.
    #[tokio::test]
    async fn rotation_and_failover_never_claim_the_same_incumbent() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");

        let healthy = std::sync::Arc::new(std::sync::Mutex::new(
            crate::mixnet::MixnetSlot::Attached(attached_client(addr)),
        ));
        let Err(refused) = super::super::install_failover_client(&healthy, attached_client(addr))
        else {
            panic!("a healthy incumbent has no failover to continue");
        };
        refused.stop().await;

        let condemned_client = attached_client(addr);
        condemned_client.condemn();
        let condemned = std::sync::Arc::new(std::sync::Mutex::new(
            crate::mixnet::MixnetSlot::Attached(condemned_client),
        ));
        let Err(refused) = super::super::install_rotated_client(&condemned, attached_client(addr))
        else {
            panic!("a condemned incumbent is the failover's to replace, not a rotation's");
        };
        refused.stop().await;
    }

    /// HYPOTHESIS: a rotation refuses a slot whose consent was revoked
    /// mid-birth, exactly as a failover does, so neither reinstates a
    /// transport the user switched off. Falsified if the install
    /// overwrites a SwitchedOff slot.
    #[tokio::test]
    async fn a_switched_off_slot_refuses_the_rotated_client() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let slot = std::sync::Arc::new(std::sync::Mutex::new(
            crate::mixnet::MixnetSlot::SwitchedOff,
        ));

        let Err(refused) = super::super::install_rotated_client(&slot, attached_client(addr))
        else {
            panic!("a switched-off slot must refuse the rotation");
        };
        refused.stop().await;

        assert!(
            matches!(
                &*slot.lock().expect("mixnet slot mutex"),
                crate::mixnet::MixnetSlot::SwitchedOff
            ),
            "the revoked consent stands"
        );
    }

    /// How long a watchdog that must not wait is given to retire.
    const IMMEDIATE_RETIREMENT: std::time::Duration = std::time::Duration::from_millis(50);

    /// HYPOTHESIS: a platform that rules rotation out retires the
    /// watchdog rather than parking it on a cadence whose answer cannot
    /// change, so a desktop session holds no sleeping task for its
    /// whole life. Falsified if the watchdog waits out a rotation
    /// interval before reading a verdict it could have read at once.
    #[tokio::test]
    async fn a_platform_that_never_rotates_retires_the_watchdog() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let slot = std::sync::Arc::new(std::sync::Mutex::new(crate::mixnet::MixnetSlot::Attached(
            attached_client(addr),
        )));
        let pools = crate::mixnet::pools::Pools::new();
        // The desktop acquirer is the production Never, and its verdict
        // is answered without the binary ever being spawned.
        pools.set_acquirer(std::sync::Arc::new(
            crate::mixnet::acquire::Acquirer::Spawned(crate::mixnet::acquire::SpawnedBinary::at(
                "nym-proxy".into(),
            )),
        ));

        let watchdog = tokio::spawn(super::super::standing_rotation_watchdog(
            slot,
            pools,
            crate::mixnet::status_publisher(),
        ));
        tokio::time::timeout(IMMEDIATE_RETIREMENT, watchdog)
            .await
            .expect("a Never verdict retires the watchdog before any cadence elapses")
            .expect("the watchdog runs to completion");
    }

    /// An acquirer whose every birth refuses, so a rotation exhausts.
    struct BirthlessAcquirer;

    impl crate::mixnet::acquire::TransportAcquirable for BirthlessAcquirer {
        async fn discover(
            &self,
        ) -> Result<std::collections::HashSet<ExitNodeId>, crate::mixnet::acquire::TransportError>
        {
            Ok(std::collections::HashSet::new())
        }

        async fn acquire(
            &self,
            _clutch: &[ExitNodeId],
            _publisher: crate::mixnet::driver::StatusPublisher,
        ) -> Result<crate::mixnet::MixnetProxy, crate::mixnet::acquire::TransportError> {
            Err(crate::mixnet::acquire::TransportError::NotReady {
                budget: zl_netutils::time::NYM_LIFECYCLE_TIMEOUT,
            })
        }

        fn rotation_verdict(&self) -> crate::mixnet::acquire::RotationVerdict {
            crate::mixnet::acquire::RotationVerdict::Now
        }
    }

    /// HYPOTHESIS: make-before-break means a rotation that cannot prove
    /// a replacement is a no-op, so the session keeps its service and
    /// publishes no transition at all. Falsified if a failed rotation
    /// leaves the slot empty or dips the mode, either of which turns a
    /// privacy measure into an outage.
    #[tokio::test]
    async fn a_rotation_without_a_replacement_keeps_the_incumbent() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let slot = std::sync::Arc::new(std::sync::Mutex::new(crate::mixnet::MixnetSlot::Attached(
            attached_client(addr),
        )));
        let pools = crate::mixnet::pools::Pools::new();
        let status = crate::mixnet::status_publisher();
        let mut watching = status.subscribe();
        watching.borrow_and_update();

        super::super::hand_off_standing_client(&slot, &pools, &status, &BirthlessAcquirer).await;

        assert!(
            matches!(
                &*slot.lock().expect("mixnet slot mutex"),
                crate::mixnet::MixnetSlot::Attached(client) if !client.is_condemned()
            ),
            "a rotation that cannot prove a replacement must keep the client it has"
        );
        assert!(
            !watching
                .has_changed()
                .expect("the channel outlives the test"),
            "a failed rotation is invisible: the incumbent never stopped serving"
        );
    }
}

mod expiry_watchdog_parks {
    //! The watchdog's progress discipline (finding 3 of the 2026-08-14
    //! PR #2695 review): an acquisition that cannot advance a lapsed
    //! proof deadline parks the watchdog on the status channel instead
    //! of spinning it against a deadline nothing will move.
    use crate::mixnet::ExitNodeId;

    /// How long the test observes the armed watchdog for starvation.
    const PARK_OBSERVATION: std::time::Duration = std::time::Duration::from_millis(50);

    /// A Standing Client over an attach to `addr`, for slot tests.
    fn attached_client(addr: std::net::SocketAddr) -> crate::mixnet::StandingClient {
        let proxy = crate::mixnet::MixnetProxy::attach(
            addr,
            &[ExitNodeId::from("exit-alpha")],
            crate::mixnet::test_indexer(),
            crate::mixnet::status_publisher(),
        )
        .expect("the attach accepts a bound exit");
        crate::mixnet::StandingClient::new(proxy, None, true)
    }

    /// HYPOTHESIS: a lapsed deadline the acquisition cannot advance — a
    /// condemned client mid-failover — parks the watchdog awaiting the
    /// slot's next published transition. Falsified if the watchdog's
    /// hot loop starves this current-thread runtime, so the sleep below
    /// never completes and the test dies by timeout.
    #[tokio::test]
    async fn a_stalled_acquisition_parks_the_watchdog() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let client = attached_client(addr);
        client.condemn();
        client.set_proof_deadline(std::time::Instant::now());
        let slot = std::sync::Arc::new(std::sync::Mutex::new(crate::mixnet::MixnetSlot::Attached(
            client,
        )));
        let pools = crate::mixnet::pools::Pools::new();
        let status = crate::mixnet::status_publisher();

        let watchdog = tokio::spawn(super::super::standing_proof_watchdog(
            slot.clone(),
            pools,
            status,
        ));

        tokio::time::sleep(PARK_OBSERVATION).await;
        assert!(
            !watchdog.is_finished(),
            "the parked watchdog still guards the slot"
        );
        watchdog.abort();
    }
}

mod birth_channel_isolation {
    //! The birth-channel rule (finding 3 of the PR #2705 review): a
    //! birth's candidate lifecycle belongs to the birth's own channel,
    //! and the session channel speaks only for the slot.
    use std::sync::{Arc, Mutex};

    use crate::lightclient::LightClient;
    use crate::mixnet::{ExitNodeId, Indicator, MixnetStatus};
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

    fn wallet() -> crate::wallet::LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build()
    }

    /// The exit this harness announces as bound, and the first the
    /// boot assigns a role.
    const HARNESS_EXIT: &str = "exit-alpha";

    /// Every exit this harness discovers. Boot proves one exit per
    /// role, so a population smaller than
    /// [`roles::ROLE_EXIT_COUNT`](crate::mixnet::roles::ROLE_EXIT_COUNT)
    /// cannot fill the roles and the enable refuses.
    const HARNESS_CENSUS: [&str; crate::mixnet::roles::ROLE_EXIT_COUNT] =
        [HARNESS_EXIT, "exit-beta", "exit-gamma"];

    /// Yields granted after a publication so a subscriber on another
    /// worker observes it before the watch channel coalesces.
    const ANNOUNCEMENT_YIELDS: usize = 8;

    /// HYPOTHESIS: during an enable the session channel narrates the
    /// slot owner's Bootstrapping and never a candidate's Ready, so a
    /// subscriber cannot observe a client the slot does not hold.
    /// Falsified if any Ready reaches the session channel mid-enable.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_enable_narrates_without_relaying_candidates() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let mut client = LightClient::new_for_test(wallet()).await;
        for exit in HARNESS_CENSUS {
            client.exit_pools.remember(
                ExitNodeId::from(exit),
                crate::mixnet::pools::exit_pool::ExitNodeHealthVerdict::EpochProven,
            );
        }
        let mut feed = client.subscribe_mixnet_status();
        let seen: Arc<Mutex<Vec<MixnetStatus>>> = Arc::default();
        let sink = Arc::clone(&seen);
        // The collector must be parked on the channel before the enable
        // begins, or coalescing hides the transients it exists to catch.
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let collector = tokio::spawn(async move {
            started_tx.send(()).expect("the test task is waiting");
            while feed.changed().await.is_ok() {
                let status = feed.borrow_and_update().clone();
                sink.lock().expect("collector mutex").push(status);
            }
        });
        started_rx.await.expect("the collector starts");

        client
            .enable_mixnet_from(std::sync::Arc::new(
                crate::mixnet::acquire::Acquirer::Announcing(
                    crate::mixnet::acquire::AnnouncingAcquirer {
                        socks5_addr: addr,
                        census: HARNESS_CENSUS
                            .iter()
                            .map(|id| ExitNodeId::from(*id))
                            .collect(),
                        yields: ANNOUNCEMENT_YIELDS,
                        // The harness pins the client it scripts, so a
                        // rotation must never replace it mid-test.
                        rotation: crate::mixnet::acquire::RotationVerdict::Never,
                    },
                ),
            ))
            .await
            .expect("the trusted birth settles the enable");

        for _ in 0..ANNOUNCEMENT_YIELDS {
            tokio::task::yield_now().await;
        }
        collector.abort();
        let seen = seen.lock().expect("collector mutex");
        assert!(
            seen.iter().all(|status| status.mode != Indicator::Ready),
            "no candidate Ready reaches the session channel: {seen:?}"
        );
        // A watch channel promises the latest state, not every event, so
        // the narration clause asserts only what any subscriber is
        // guaranteed: whatever it observes first is the slot owner's
        // plain Bootstrapping, never a candidate's detailed one.
        if let Some(first) = seen.first() {
            assert_eq!(
                first.mode,
                Indicator::Bootstrapping,
                "the channel's first mid-enable state is the slot owner's"
            );
            assert_eq!(
                first.bootstrap_detail, None,
                "the narration is the slot owner's, never a candidate's"
            );
        }
    }
}

mod death_story {
    //! A latched death reaches subscribers with its typed cause (finding
    //! F6 of the PR #2705 review, item 2 of the 2026-08-14 split).
    use crate::mixnet::{ExitNodeId, Indicator};

    /// HYPOTHESIS: a forsaken Standing Client's Died reaches the status
    /// channel carrying the exhaustion's typed story, so a consumer
    /// renders the cause and never a bare "died". Falsified if the
    /// published death is None.
    #[tokio::test]
    async fn a_forsaken_client_publishes_its_death_story() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let birth_channel = crate::mixnet::status_publisher();
        let proxy = crate::mixnet::MixnetProxy::attach(
            addr,
            &[ExitNodeId::from("exit-alpha")],
            crate::mixnet::test_indexer(),
            birth_channel,
        )
        .expect("the attach accepts a bound exit");
        let client = crate::mixnet::StandingClient::new(proxy, None, true);
        let exhausted = crate::mixnet::acquire::TransportError::NoProvenExit {
            probed: crate::mixnet::pools::MAX_PROVING_BIRTHS,
            budget: zl_netutils::time::SENTINEL_BUDGET,
        };
        client.forsake(zl_netutils::diag::NetOpFailure::from_error(
            zl_netutils::diag::NetOpStage::RouteResolution,
            "the session's exit census",
            &exhausted,
        ));
        let slot = std::sync::Arc::new(std::sync::Mutex::new(crate::mixnet::MixnetSlot::Attached(
            client,
        )));

        let session = crate::mixnet::status_publisher();
        super::super::publish_slot(&slot, &session);

        let published = session.subscribe().borrow().clone();
        assert_eq!(published.mode, Indicator::Died, "forsaken latches Died");
        let death = published
            .death
            .expect("the published death carries its report");
        let detail = death.detail.expect("the report holds the typed cause");
        assert!(
            detail
                .cause_chain
                .iter()
                .any(|layer| layer.contains("no proven exit")),
            "the exhaustion's own words reach the subscriber: {detail:?}"
        );
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

    /// - Binds a lazy Session Indexer on the client without dialing it.
    async fn client_with_session_indexer() -> LightClient {
        let mut client = LightClient::new_for_test(wallet()).await;
        client
            .set_indexer_uri_lazy(
                "https://indexer.test:443"
                    .parse()
                    .expect("the literal is an absolute https URI with a port"),
            )
            .expect("a lazy indexer binds an absolute https URI without dialing it");
        client
    }

    /// HYPOTHESIS: a failed enable hands the session back as the user
    /// had it: from `SwitchedOff` under the nakednet policy, a failed
    /// `attach_mixnet` lands `Unattached` and publishes it, and the
    /// policy is `Nakednet` again, so the user who chose nakednet keeps
    /// sending there after an enable that did not take. Falsified if
    /// the mode remains `SwitchedOff` or the policy ends elsewhere.
    #[tokio::test]
    async fn a_failed_attach_restores_the_policy_the_user_had() {
        let mut client = client_with_session_indexer().await;
        client.disable_mixnet().await;
        client.set_transmit_policy(crate::mixnet::TransmitPolicy::Nakednet);
        let subscriber = client.subscribe_mixnet_status();

        client
            .attach_mixnet("not-an-address", &[])
            .await
            .expect_err("an unparseable mobile platform address must fail the attach");

        assert_eq!(
            client.read_mixnet_indicator(),
            crate::mixnet::Indicator::Unattached
        );
        assert_eq!(
            subscriber.borrow().mode,
            crate::mixnet::Indicator::Unattached,
            "subscribers must see the settled state"
        );
        assert_eq!(
            client.transmit_policy(),
            crate::mixnet::TransmitPolicy::Nakednet,
            "a failed enable must hand the policy back"
        );
        assert_eq!(
            client.send_route(),
            Ok(crate::mixnet::MixnetRoute::Nakednet),
            "the user who chose nakednet keeps sending there"
        );
    }

    /// HYPOTHESIS: the enable is the mixnet consent from the moment it
    /// is asked for, so a send during the bootstrap refuses rather than
    /// travelling the nakednet route the user just turned away from.
    /// Attaches to a refusing localhost port from a nakednet session:
    /// the attach is accepted, the readiness gate runs, and the send
    /// route reads `Bootstrapping` while it does. Falsified if the send
    /// route yields nakednet at any point after the attach.
    #[tokio::test]
    async fn a_send_during_the_bootstrap_refuses_rather_than_leak() {
        let mut client = client_with_session_indexer().await;
        client.set_transmit_policy(crate::mixnet::TransmitPolicy::Nakednet);
        assert_eq!(
            client.send_route(),
            Ok(crate::mixnet::MixnetRoute::Nakednet)
        );

        client
            .attach_mixnet(
                "127.0.0.1:9",
                &[crate::mixnet::ExitNodeId::from("exit-alpha")],
            )
            .await
            .expect("a well-formed address and a named exit attach");

        assert_eq!(
            client.transmit_policy(),
            crate::mixnet::TransmitPolicy::Mixnet,
            "the attach itself is the consent, before readiness"
        );
        assert!(
            matches!(
                client.send_route(),
                Err(crate::mixnet::MixnetNotReady::Bootstrapping
                    | crate::mixnet::MixnetNotReady::Died)
            ),
            "a send after the attach must refuse until the gate settles, got {:?}",
            client.send_route()
        );
        client.go_offline().await;
    }

    /// HYPOTHESIS: a failed enable on a session that never chose
    /// nakednet keeps refusing sends: the default `Mixnet` policy
    /// stands and the slot is `Unattached`. Falsified if the failure
    /// opens a nakednet send.
    #[tokio::test]
    async fn a_failed_attach_without_a_nakednet_choice_still_refuses() {
        let mut client = LightClient::new_for_test(wallet()).await;

        client
            .attach_mixnet("not-an-address", &[])
            .await
            .expect_err("an unparseable mobile platform address must fail the attach");

        assert_eq!(
            client.send_route(),
            Err(crate::mixnet::MixnetNotReady::Unattached)
        );
    }

    /// HYPOTHESIS: a runtime enable after the startup opt-out brings
    /// sends back to the mixnet: the policy the opt-out set to
    /// `Nakednet` is `Mixnet` again once the enable settles. Falsified
    /// if a ready transport is paired with a nakednet send route.
    #[tokio::test]
    async fn enabling_after_the_opt_out_routes_sends_over_the_mixnet() {
        let mut client = LightClient::new_for_test(wallet()).await;
        client
            .start_mixnet_session(
                crate::mixnet::ProvisionStrategy::Spawn(
                    crate::mixnet::provision::SpawnHints::default(),
                ),
                crate::mixnet::MixnetStartPolicy::OptedOutThisSession,
            )
            .await
            .expect("the opt-out provisions nothing and cannot fail");
        assert_eq!(
            client.transmit_policy(),
            crate::mixnet::TransmitPolicy::Nakednet
        );

        let socks5_addr = crate::mocks::transmission::MOCK_SOCKS5_ADDR;
        client.attach_mixnet_for_tests(socks5_addr, true).await;

        assert!(client.read_mixnet_indicator().is_ready());
        assert_eq!(
            client.transmit_policy(),
            crate::mixnet::TransmitPolicy::Mixnet
        );
        assert!(
            matches!(
                client.send_route(),
                Ok(crate::mixnet::MixnetRoute::Mixnet(_))
            ),
            "a ready transport after an enable must carry the sends"
        );
    }

    /// The driver entry honors the startup opt-out (ADR 0024, consent
    /// at start): OptedOutThisSession lands SwitchedOff without
    /// provisioning anything — the strategy is never exercised — sets
    /// the transmit policy to nakednet so sends still travel, and the
    /// transition reaches subscribers through the session channel.
    #[tokio::test]
    async fn the_driver_records_the_startup_opt_out_and_publishes_it() {
        let mut client = LightClient::new_for_test(wallet()).await;
        let subscriber = client.subscribe_mixnet_status();
        assert_eq!(
            subscriber.borrow().mode,
            crate::mixnet::Indicator::Unattached,
            "the channel opens in the ground state"
        );
        assert_eq!(
            client.transmit_policy(),
            crate::mixnet::TransmitPolicy::Mixnet,
            "the session starts under the mixnet policy"
        );

        client
            .start_mixnet_session(
                // A hint set that resolves to no real binary: the
                // opt-out branch must never try to spawn it.
                crate::mixnet::ProvisionStrategy::Spawn(
                    crate::mixnet::provision::SpawnHints::default(),
                ),
                crate::mixnet::MixnetStartPolicy::OptedOutThisSession,
            )
            .await
            .expect("the opt-out provisions nothing and cannot fail");

        assert_eq!(
            client.read_mixnet_indicator(),
            crate::mixnet::Indicator::SwitchedOff
        );
        assert_eq!(
            subscriber.borrow().mode,
            crate::mixnet::Indicator::SwitchedOff,
            "the slot transition must reach subscribers"
        );
        assert_eq!(
            client.transmit_policy(),
            crate::mixnet::TransmitPolicy::Nakednet,
            "the opt-out is the session's nakednet consent"
        );
        assert_eq!(
            client.send_route(),
            Ok(crate::mixnet::MixnetRoute::Nakednet),
            "an opted-out session must still transmit"
        );
        assert_eq!(
            client.mixnet_only_route().err(),
            Some(crate::mixnet::MixnetNotReady::Unattached),
            "the opt-out never opens a nakednet price fetch"
        );
    }

    /// A forced-on attach to a malformed address fails typed and leaves
    /// Unattached — refusal, never nakednet — and publishes the settled
    /// state so a subscriber cannot be left staring at a stale mode.
    #[tokio::test]
    async fn a_failed_forced_on_start_publishes_unattached() {
        let mut client = client_with_session_indexer().await;
        let subscriber = client.subscribe_mixnet_status();

        let error = client
            .start_mixnet_session(
                crate::mixnet::ProvisionStrategy::Attach {
                    socks5_addr: "not-a-socket-address",
                    exits: &[],
                },
                crate::mixnet::MixnetStartPolicy::ForcedOn,
            )
            .await
            .expect_err("a malformed attach address must refuse");
        assert!(matches!(
            error,
            crate::mixnet::SessionStartError::Attach {
                source: crate::mixnet::MixnetProxyError::InvalidAddress { .. },
                ..
            }
        ));

        assert_eq!(
            client.read_mixnet_indicator(),
            crate::mixnet::Indicator::Unattached
        );
        assert_eq!(
            subscriber.borrow().mode,
            crate::mixnet::Indicator::Unattached,
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
        let mut client = client_with_session_indexer().await;
        let mut subscriber = client.subscribe_mixnet_status();

        client
            .start_mixnet_session(
                // Port 9 (discard) refuses: readiness fails fast and
                // the driver lands Died.
                crate::mixnet::ProvisionStrategy::Attach {
                    socks5_addr: "127.0.0.1:9",
                    exits: &[crate::mixnet::ExitNodeId::from("host-bound-exit")],
                },
                crate::mixnet::MixnetStartPolicy::ForcedOn,
            )
            .await
            .expect("a well-formed address attaches");

        let died = subscriber
            .wait_for(|status| status.mode == crate::mixnet::Indicator::Died)
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
            crate::mixnet::Indicator::SwitchedOff
        );
        tokio::task::yield_now().await;
        assert!(
            !subscriber.has_changed().expect("the publisher is alive"),
            "no stale transport publication may follow the deliberate disable"
        );
    }

    /// HYPOTHESIS: the test-only mixnet switch takes an already parsed
    /// socket address, so the contract is checked by the compiler and no
    /// external harness can abort inside it on a placeholder string.
    /// Falsified if the helper still takes text and parses within.
    #[tokio::test]
    async fn the_test_switch_takes_a_typed_socket_address() {
        let mut client = LightClient::new_for_test(wallet()).await;
        let socks5_addr = crate::mocks::transmission::MOCK_SOCKS5_ADDR;

        client.switch_on_mixnet_for_tests(socks5_addr).await;

        assert_eq!(
            client.mixnet_socks5_addr(),
            Some(socks5_addr),
            "the slot reports the very address the caller handed it"
        );
    }

    /// The recovery predicate is Died only: the ground state carries no
    /// online intent (a wallet may never have consented to
    /// connectivity), switched off is consent revocation's territory,
    /// and the live states need no repair. Exhaustive over ALL so a new
    /// state must take a position.
    #[test]
    fn the_recovery_predicate_is_died_only() {
        for mode in crate::mixnet::Indicator::ALL {
            assert_eq!(
                mode.needs_recovery(),
                matches!(mode, crate::mixnet::Indicator::Died),
                "{mode} must {}need recovery",
                if matches!(mode, crate::mixnet::Indicator::Died) {
                    ""
                } else {
                    "not "
                }
            );
        }
    }
}

//! Mixnet Mode toggle (ADR 0011, consumption model A). Enabling spawns the
//! bundled `nym-proxy` child process. Disabling shuts it down. The mode
//! reflects the transport slot's state, and nakednet is reachable only by a
//! deliberate disable, never as a silent fallback: a session that never
//! enabled the mixnet, or whose enable failed, is `Unattached` and refuses.

#![forbid(unsafe_code)]

use super::error::LightClientError;
use super::{LightClient, MixnetPriceFetch};

/// The price run as a speed-priority operation: it races the price census
/// through one Exit Node and settles on the first quote, whichever operator
/// answers.
#[cfg(feature = "nym")]
pub(crate) struct PriceRun {
    /// The pools this run takes its transport from and returns it to.
    pools: std::sync::Arc<crate::mixnet::pools::Pools>,
}

#[cfg(feature = "nym")]
impl crate::mixnet::speed::SpeedPrioritized for PriceRun {
    type Target = crate::price::PriceSource;
    type Outcome = (
        crate::price::PriceSource,
        Result<zl_common::price::Price, crate::price::PriceError>,
    );

    fn targets(&self) -> Vec<Self::Target> {
        crate::price::RACED_SOURCES.to_vec()
    }

    async fn probe(
        &self,
        dial: zl_netutils::conduit::ConduitDial,
        target: Self::Target,
    ) -> Self::Outcome {
        let outcome = crate::price::get_source_price(
            target,
            &dial,
            target.url(),
            crate::price::REQUEST_TIMEOUT,
            crate::price::CONNECT_TIMEOUT,
        )
        .await;
        (target, outcome)
    }

    fn settled(&self, outcomes: &[Self::Outcome]) -> bool {
        outcomes.iter().any(|(_, outcome)| outcome.is_ok())
    }

    fn answered(&self, outcomes: &[Self::Outcome]) -> bool {
        outcomes.iter().any(|(_, outcome)| outcome.is_ok())
    }

    fn acquire(
        &self,
    ) -> impl std::future::Future<
        Output = Result<
            (crate::mixnet::speed::Member, std::net::SocketAddr),
            crate::mixnet::acquire::TransportError,
        >,
    > + Send {
        // Boot's price fetch runs over the conduit proved for its role, so
        // the first quote costs no birth. A later run finds the conduit
        // spent and births its own client, which keeps priced traffic off
        // the wallet-correlated streams on the standing client.
        let pools = self.pools.clone();
        async move {
            let acquirer = pools
                .acquirer()
                .ok_or(crate::mixnet::acquire::TransportError::NoAcquirer)?;
            let birth = pools
                .conduit_for(crate::mixnet::roles::Role::PriceFetch, acquirer.as_ref())
                .await?;
            let member = crate::mixnet::speed::Member::new(birth.transport, birth.lease);
            let addr = member
                .addr()
                .ok_or(crate::mixnet::acquire::TransportError::DiedBeforeUse)?;
            Ok((member, addr))
        }
    }

    fn dispose(&self, spent: crate::mixnet::speed::Member) {
        // The quote is a completed round trip, so the exit's proof renews;
        // the client still turns off behind the run, because consecutive
        // fetches must not share a client.
        self.pools.remember(
            spent.node().clone(),
            crate::mixnet::pools::exit_pool::ExitNodeHealthVerdict::EpochProven,
        );
        tokio::spawn(async move {
            spent.retire().await;
        });
    }

    fn abandon(&self, dead: crate::mixnet::speed::Member) {
        self.pools.remember(
            dead.node().clone(),
            crate::mixnet::pools::exit_pool::ExitNodeHealthVerdict::Failed,
        );
        tokio::spawn(async move {
            dead.retire().await;
        });
    }

    fn narrate(&self, phase: crate::mixnet::speed::SpeedProgress) {
        tracing::info!("price run: {phase:?}");
    }
}

/// Replaces the slot's contents under its lock, yielding what it held.
fn swap_slot(
    slot: &std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    next: crate::mixnet::MixnetSlot,
) -> crate::mixnet::MixnetSlot {
    std::mem::replace(&mut *slot.lock().expect("mixnet slot mutex"), next)
}

/// Publishes the slot's settled state into `status`, read under one lock.
fn publish_slot(
    slot: &std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    status: &crate::mixnet::driver::StatusPublisher,
) {
    let guarded = slot.lock().expect("mixnet slot mutex");
    // The evidenced constructor keeps only what the mode offers, so the
    // condemned dip publishes no stale address and a latched death crosses
    // whole for subscribers to render its typed story.
    status.send_replace(crate::mixnet::MixnetStatus::evidenced(
        guarded.mode(),
        guarded.socks5_addr(),
        guarded.exits(),
        guarded.death_report(),
    ));
}

/// Forwards a settled birth's later transitions from its birth channel into
/// the session channel, ending when the client's driver drops its publisher.
fn forward_client_transitions(
    birth_channel: &crate::mixnet::driver::StatusPublisher,
    session: &crate::mixnet::driver::StatusPublisher,
) {
    let mut future_transitions = birth_channel.subscribe();
    let session = std::sync::Arc::clone(session);
    tokio::spawn(async move {
        while future_transitions.changed().await.is_ok() {
            let status = future_transitions.borrow_and_update().clone();
            session.send_replace(status);
        }
    });
}

/// Installs the failover's replacement only while the slot still holds the
/// condemned client whose enable it continues, refusing when consent
/// changed shape mid-birth.
// The Err carries the refused client back to its caller for teardown, one
// move that never rides an error chain, so its size costs nothing.
#[allow(clippy::result_large_err)]
fn install_failover_client(
    slot: &std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    client: crate::mixnet::StandingClient,
) -> Result<crate::mixnet::MixnetSlot, crate::mixnet::StandingClient> {
    let mut guarded = slot.lock().expect("mixnet slot mutex");
    match &*guarded {
        crate::mixnet::MixnetSlot::Attached(current) if current.is_condemned() => Ok(
            std::mem::replace(&mut *guarded, crate::mixnet::MixnetSlot::Attached(client)),
        ),
        _ => Err(client),
    }
}

/// Installs a rotation's replacement only while the slot still holds the
/// healthy client the rotation set out to replace, refusing when a failover
/// or a teardown reached the slot first.
// The exact complement of `install_failover_client`: that path requires a
// condemned incumbent and this one requires an uncondemned one, so a
// rotation and a failover racing the same slot cannot both install.
#[allow(clippy::result_large_err)]
fn install_rotated_client(
    slot: &std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    client: crate::mixnet::StandingClient,
) -> Result<crate::mixnet::MixnetSlot, crate::mixnet::StandingClient> {
    let mut guarded = slot.lock().expect("mixnet slot mutex");
    match &*guarded {
        crate::mixnet::MixnetSlot::Attached(current) if !current.is_condemned() => Ok(
            std::mem::replace(&mut *guarded, crate::mixnet::MixnetSlot::Attached(client)),
        ),
        _ => Err(client),
    }
}

/// Builds the Standing Client a birth installs, its proof deadline set to
/// the earliest feasible ProofAcquisition moment: one epoch from a probed
/// birth's own answer, or the stale observation's original expiry for a
/// trusting birth.
fn standing_client_from_birth(
    birth: crate::mixnet::pools::ProvenBirth,
) -> crate::mixnet::StandingClient {
    use crate::mixnet::pools::Proof;

    let proof = birth.proof;
    let client = crate::mixnet::StandingClient::new(
        birth.transport,
        Some(birth.lease),
        matches!(proof, Proof::Earned { .. }),
    );
    // An inherited proof carries its own deadline, so the watchdog fires
    // when the observation the birth trusted ages out rather than an epoch
    // from now.
    if let Proof::Inherited { proven_until } = proof {
        client.set_proof_deadline(proven_until);
    }
    client
}

/// One ProofAcquisition over the Standing Client: adjudicate `evidence`
/// about its exit, promoting on an answer and, on silence, convicting the
/// exit and running the two-layer failover — Bootstrapping while a
/// replacement births, Died latched when every birth exhausts.
async fn adjudicate_standing_proof(
    slot: std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    pools: std::sync::Arc<crate::mixnet::pools::Pools>,
    status: crate::mixnet::driver::StatusPublisher,
    evidence: zl_netutils::sentinel::ExitEvidence,
) {
    // Read the client's facts under a brief lock; never hold it across an
    // await.
    let (node, convicted) = {
        let guarded = slot.lock().expect("mixnet slot mutex");
        let crate::mixnet::MixnetSlot::Attached(client) = &*guarded else {
            return;
        };
        if evidence.proves_the_exit() {
            client.note_round_trip();
            let refreshed = client.exit_node().cloned();
            drop(guarded);
            if let Some(node) = refreshed {
                pools.remember(
                    node,
                    crate::mixnet::pools::exit_pool::ExitNodeHealthVerdict::EpochProven,
                );
            }
            publish_slot(&slot, &status);
            return;
        }
        (client.exit_node().cloned(), client.condemn())
    };
    if !convicted {
        // Another adjudication already owns the failover.
        return;
    }
    if let Some(node) = node {
        pools.remember(
            node,
            crate::mixnet::pools::exit_pool::ExitNodeHealthVerdict::Failed,
        );
    }
    // The ruled Bootstrapping dip: the session cannot truthfully claim
    // readiness while the replacement births.
    publish_slot(&slot, &status);
    let Some(acquirer) = pools.acquirer() else {
        // An attached session cannot rebirth locally; the loss surfaces to
        // the host as Died, per the two-layer ruling's exhaustion arm.
        if let crate::mixnet::MixnetSlot::Attached(client) =
            &*slot.lock().expect("mixnet slot mutex")
        {
            client.forsake(zl_netutils::diag::NetOpFailure::message(
                zl_netutils::diag::NetOpStage::RouteResolution,
                "the attached transport",
                "the exit failed under an attached session, which cannot \
                 rebirth its transport locally",
            ));
        }
        publish_slot(&slot, &status);
        return;
    };
    match pools.acquire_proven(acquirer.as_ref()).await {
        Ok(birth) => {
            let birth_channel = std::sync::Arc::clone(&birth.lifecycle);
            let client = standing_client_from_birth(birth);
            match install_failover_client(&slot, client) {
                Ok(replaced) => {
                    // The replacement's candidate churn stayed on its birth
                    // channel; its settled transitions reach the session's
                    // subscribers from here on.
                    forward_client_transitions(&birth_channel, &status);
                    publish_slot(&slot, &status);
                    if let crate::mixnet::MixnetSlot::Attached(old) = replaced {
                        // The replacement is installed, so the session is
                        // whole; the outgoing client keeps its transport
                        // open only for work already dialed through it.
                        old.retire().await;
                    }
                }
                Err(refused) => {
                    // Consent changed shape mid-birth: the enable this
                    // failover continues is gone, so the replacement dies
                    // unused and its lease recycles.
                    refused.stop().await;
                }
            }
        }
        Err(exhausted) => {
            log::warn!("the standing client's failover exhausted: {exhausted}");
            if let crate::mixnet::MixnetSlot::Attached(client) =
                &*slot.lock().expect("mixnet slot mutex")
            {
                client.forsake(zl_netutils::diag::NetOpFailure::from_error(
                    zl_netutils::diag::NetOpStage::RouteResolution,
                    "the session's exit census",
                    &exhausted,
                ));
            }
            publish_slot(&slot, &status);
        }
    }
}

/// Runs one ProofAcquisition end to end: the arbiter Sentinel exchange
/// dialed into the Standing Client's tunnel, then the adjudication.
async fn run_proof_acquisition(
    slot: std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    pools: std::sync::Arc<crate::mixnet::pools::Pools>,
    status: crate::mixnet::driver::StatusPublisher,
) {
    let socks5 = {
        let guarded = slot.lock().expect("mixnet slot mutex");
        match &*guarded {
            crate::mixnet::MixnetSlot::Attached(client) if !client.is_condemned() => {
                match client.proxy().socks5_addr() {
                    Some(addr) => addr,
                    None => return,
                }
            }
            _ => return,
        }
    };
    let evidence =
        zl_netutils::sentinel::probe_sentinel(socks5, zl_netutils::time::SENTINEL_BUDGET).await;
    adjudicate_standing_proof(slot, pools, status, evidence).await;
}

/// The expiry watchdog: sleeps until the Standing Client's proof deadline,
/// fires a new ProofAcquisition at that earliest feasible moment, follows
/// replacement clients in the slot until the slot empties, and parks on the
/// status channel whenever an acquisition leaves a lapsed deadline
/// unadvanced.
async fn standing_proof_watchdog(
    slot: std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    pools: std::sync::Arc<crate::mixnet::pools::Pools>,
    status: crate::mixnet::driver::StatusPublisher,
) {
    let mut wake = status.subscribe();
    loop {
        let deadline = {
            let guarded = slot.lock().expect("mixnet slot mutex");
            match &*guarded {
                crate::mixnet::MixnetSlot::Attached(client) => client.proof_deadline(),
                _ => return,
            }
        };
        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        let due = {
            let guarded = slot.lock().expect("mixnet slot mutex");
            match &*guarded {
                crate::mixnet::MixnetSlot::Attached(client) => {
                    // A round trip may have moved the deadline while we
                    // slept; only a still-lapsed proof is due.
                    client.proof_deadline() <= std::time::Instant::now()
                }
                _ => return,
            }
        };
        if due {
            run_proof_acquisition(slot.clone(), pools.clone(), status.clone()).await;
            // Mark the channel seen before the progress read, so an install
            // landing after the read still wakes the park below.
            wake.borrow_and_update();
            let advanced = {
                let guarded = slot.lock().expect("mixnet slot mutex");
                match &*guarded {
                    crate::mixnet::MixnetSlot::Attached(client) => {
                        client.proof_deadline() > deadline
                    }
                    _ => return,
                }
            };
            // An acquisition that could not act — a condemned client
            // mid-failover, a forsaken exhaustion, an address-less proxy —
            // left the lapsed deadline in place, and re-arming on it would
            // spin a core until vacate: park until the slot's next
            // published transition instead.
            if !advanced && wake.changed().await.is_err() {
                return;
            }
        }
    }
}

/// One rotation, make-before-break: prove a replacement while the incumbent
/// still serves, install it, then retire the incumbent once the work already
/// dialed through it drains (ADR 0048).
async fn hand_off_standing_client<A>(
    slot: &std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    pools: &std::sync::Arc<crate::mixnet::pools::Pools>,
    status: &crate::mixnet::driver::StatusPublisher,
    acquirer: &A,
) where
    A: crate::mixnet::acquire::TransportAcquirable + ?Sized,
{
    // The replacement births on its own channel and nothing publishes until
    // it is installed, so a rotation is never visible as a bootstrap: the
    // incumbent is serving throughout.
    let birth = match pools.acquire_proven(acquirer).await {
        Ok(birth) => birth,
        Err(exhausted) => {
            // A rotation that cannot prove a replacement is a no-op. The
            // incumbent is healthy, so keeping it costs only the bounded
            // exposure this rotation meant to end.
            log::warn!(
                "a rotation found no proven replacement, so the session keeps its client: {exhausted}"
            );
            return;
        }
    };
    let birth_channel = std::sync::Arc::clone(&birth.lifecycle);
    let client = standing_client_from_birth(birth);
    match install_rotated_client(slot, client) {
        Ok(replaced) => {
            forward_client_transitions(&birth_channel, status);
            publish_slot(slot, status);
            if let crate::mixnet::MixnetSlot::Attached(old) = replaced {
                old.retire().await;
            }
        }
        Err(refused) => {
            // A failover or a teardown reached the slot while the
            // replacement was proving, so the client it would have replaced
            // is already gone; the replacement dies unused and its lease
            // recycles.
            refused.stop().await;
        }
    }
}

/// The rotation watchdog: draws a cadence, waits out the platform's verdict
/// on affording a bootstrap, hands the session to a replacement, and repeats
/// until the slot empties.
async fn standing_rotation_watchdog(
    slot: std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    pools: std::sync::Arc<crate::mixnet::pools::Pools>,
    status: crate::mixnet::driver::StatusPublisher,
) {
    // Asked once before any waiting, so a platform that will never rotate
    // holds no sleeping task for the session's life.
    if let Some(acquirer) = pools.acquirer()
        && matches!(
            crate::mixnet::acquire::TransportAcquirable::rotation_verdict(acquirer.as_ref()),
            crate::mixnet::acquire::RotationVerdict::Never
        )
    {
        return;
    }
    loop {
        // The generator is scoped so no non-Send handle crosses the await.
        let cadence = {
            let mut entropy = rand::rng();
            crate::mixnet::acquire::rotation_interval(&mut entropy)
        };
        tokio::time::sleep(cadence).await;

        let acquirer = loop {
            if !matches!(
                &*slot.lock().expect("mixnet slot mutex"),
                crate::mixnet::MixnetSlot::Attached(_)
            ) {
                return;
            }
            let Some(acquirer) = pools.acquirer() else {
                // An attached session births nothing locally, so it also
                // rotates nothing.
                return;
            };
            // Asked at the moment of acting rather than when the cadence was
            // drawn, because the battery and the foreground state the host
            // weighs are the ones it has now.
            match crate::mixnet::acquire::TransportAcquirable::rotation_verdict(acquirer.as_ref()) {
                crate::mixnet::acquire::RotationVerdict::Now => break acquirer,
                crate::mixnet::acquire::RotationVerdict::Defer(after) => {
                    tokio::time::sleep(after).await
                }
                // A platform can rule rotation out mid-session, and the
                // ruling holds for the rest of it.
                crate::mixnet::acquire::RotationVerdict::Never => return,
            }
        };
        hand_off_standing_client(&slot, &pools, &status, acquirer.as_ref()).await;
    }
}

impl LightClient {
    /// Take whatever transport the slot holds and shut it down, leaving the
    /// `Unattached` that a failed enable also deliberately leaves behind: a
    /// failure must not silently reinstate a prior `SwitchedOff`. The
    /// transmit policy is not this method's to touch.
    pub(super) async fn vacate_mixnet_slot(&mut self) {
        self.exit_pools.clear_acquirer();
        // Boot's unspent conduits go with the session: a teardown before
        // their jobs took them must leave no proxy behind.
        for conduit in self.exit_pools.drain_conduits() {
            conduit.transport.stop().await;
        }
        for watchdog in [&mut self.standing_watchdog, &mut self.rotation_watchdog] {
            if let Some(watchdog) = watchdog.take() {
                watchdog.abort();
            }
        }
        if let Some(acquisition) = self
            .proof_acquisition
            .lock()
            .expect("the proof-acquisition slot is never poisoned")
            .take()
        {
            acquisition.abort();
        }
        if let crate::mixnet::MixnetSlot::Attached(client) =
            swap_slot(&self.mixnet_slot, crate::mixnet::MixnetSlot::Unattached)
        {
            // Stopping the Standing Client recycles its exit's lease by
            // drop after the transport is gone.
            client.stop().await;
        }
    }

    /// Starts the expiry and rotation watchdogs for a freshly installed
    /// Standing Client, replacing any predecessors.
    fn arm_standing_watchdog(&mut self) {
        for watchdog in [&mut self.standing_watchdog, &mut self.rotation_watchdog] {
            if let Some(watchdog) = watchdog.take() {
                watchdog.abort();
            }
        }
        self.standing_watchdog = Some(tokio::spawn(standing_proof_watchdog(
            self.mixnet_slot.clone(),
            self.exit_pools.clone(),
            std::sync::Arc::clone(&self.mixnet_status),
        )));
        // Both watchdogs follow the slot rather than a client, so a rotation
        // or a failover installing a replacement re-arms neither.
        self.rotation_watchdog = Some(tokio::spawn(standing_rotation_watchdog(
            self.mixnet_slot.clone(),
            self.exit_pools.clone(),
            std::sync::Arc::clone(&self.mixnet_status),
        )));
    }

    /// Raises the suspicion that the Standing Client's exit is dead,
    /// spawning a ProofAcquisition whose arbiter probe adjudicates it.
    pub(crate) fn note_standing_exit_suspicion(&self) {
        let mut held = self
            .proof_acquisition
            .lock()
            .expect("the proof-acquisition slot is never poisoned");
        // Single flight: a running acquisition already owns the adjudication,
        // and tracking the handle lets a vacate abort it, so a consent
        // revoked mid-birth is never raced by an untracked task.
        if held.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        *held = Some(tokio::spawn(run_proof_acquisition(
            self.mixnet_slot.clone(),
            self.exit_pools.clone(),
            std::sync::Arc::clone(&self.mixnet_status),
        )));
    }

    /// ```
    /// use zingolib::config::{ClientConfig, WalletConfig};
    /// use zingolib::lightclient::LightClient;
    /// use zingolib::mixnet::{Indicator, TransportError};
    ///
    /// tokio::runtime::Runtime::new().unwrap().block_on(async {
    ///     let wallet_dir = tempfile::tempdir().unwrap();
    ///     let config = ClientConfig::builder()
    ///         .set_wallet_dir(wallet_dir.path().to_path_buf())
    ///         .set_wallet_config(WalletConfig::MnemonicPhrase {
    ///             mnemonic_phrase: "abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon art"
    ///                 .to_string(),
    ///             no_of_accounts: std::num::NonZeroU32::new(1).unwrap(),
    ///             birthday: 2_000_000,
    ///             wallet_settings: Default::default(),
    ///         })
    ///         .build()
    ///         .unwrap();
    ///     let mut client = LightClient::new(config, true).await.unwrap();
    ///
    ///     // Enabling births the standing client from the binary at the
    ///     // given path; a binary that cannot run refuses typed at the
    ///     // first step (the exit-directory discovery), and the failed
    ///     // enable leaves Unattached — a refusal, never nakednet.
    ///     let missing = std::path::Path::new("/nonexistent/nym-proxy");
    ///     let refused = client.enable_mixnet(missing).await;
    ///     assert!(matches!(
    ///         refused,
    ///         Err(TransportError::DiscoverySpawn(_))
    ///     ));
    ///     assert_eq!(client.read_mixnet_indicator(), Indicator::Unattached);
    /// });
    /// ```
    pub async fn enable_mixnet(
        &mut self,
        binary_path: &std::path::Path,
    ) -> Result<(), crate::mixnet::acquire::TransportError> {
        self.enable_mixnet_from(std::sync::Arc::new(
            crate::mixnet::acquire::Acquirer::Spawned(crate::mixnet::acquire::SpawnedBinary::at(
                binary_path.to_path_buf(),
            )),
        ))
        .await
    }

    /// ```
    /// use zingolib::config::{ClientConfig, WalletConfig};
    /// use zingolib::lightclient::LightClient;
    /// use zingolib::mixnet::acquire::{HostRefusal, HostedTransport, ProxyHosting, RotationVerdict};
    /// use zingolib::mixnet::{ExitNodeId, Indicator, MixnetProxyError, TransportError};
    ///
    /// // A host that declines every request, standing in for the platform
    /// // that owns the proxy where subprocesses are forbidden.
    /// struct DecliningHost;
    ///
    /// impl ProxyHosting for DecliningHost {
    ///     fn discover_exit_nodes(&self) -> Result<Vec<ExitNodeId>, HostRefusal> {
    ///         Err(HostRefusal::Declined {
    ///             detail: "the platform is in low-power mode".to_string(),
    ///         })
    ///     }
    ///
    ///     fn start_transport(
    ///         &self,
    ///         _clutch: &[ExitNodeId],
    ///     ) -> Result<HostedTransport, HostRefusal> {
    ///         Err(HostRefusal::Declined {
    ///             detail: "the platform is in low-power mode".to_string(),
    ///         })
    ///     }
    ///
    ///     // Low power is exactly when a platform declines to spend a
    ///     // bootstrap, so this host holds whatever it has.
    ///     fn rotation_verdict(&self) -> RotationVerdict {
    ///         RotationVerdict::Defer(zingolib::mixnet::mixnet_timing().client_rotation_max)
    ///     }
    /// }
    ///
    /// tokio::runtime::Runtime::new().unwrap().block_on(async {
    ///     let wallet_dir = tempfile::tempdir().unwrap();
    ///     let config = ClientConfig::builder()
    ///         .set_wallet_dir(wallet_dir.path().to_path_buf())
    ///         .set_wallet_config(WalletConfig::MnemonicPhrase {
    ///             mnemonic_phrase: "abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon art"
    ///                 .to_string(),
    ///             no_of_accounts: std::num::NonZeroU32::new(1).unwrap(),
    ///             birthday: 2_000_000,
    ///             wallet_settings: Default::default(),
    ///         })
    ///         .build()
    ///         .unwrap();
    ///     let mut client = LightClient::new(config, true).await.unwrap();
    ///
    ///     // An Indexerless session has nothing for the attach readiness
    ///     // round trip to reach, so the enable refuses before it asks the
    ///     // host for anything (zingo-adrs zingolib/0063).
    ///     let indexerless = client.enable_mixnet_via_host(DecliningHost).await;
    ///     assert!(matches!(
    ///         indexerless,
    ///         Err(TransportError::Proxy(MixnetProxyError::NoSessionIndexer))
    ///     ));
    ///     client
    ///         .set_indexer_uri_lazy("https://indexer.test:443".parse().unwrap())
    ///         .unwrap();
    ///
    ///     // The standing client births from the host instead of a spawned
    ///     // binary; the host's refusal surfaces typed and the failed
    ///     // enable leaves Unattached — a refusal, never nakednet.
    ///     let refused = client
    ///         .enable_mixnet_via_host(DecliningHost)
    ///         .await;
    ///     assert!(matches!(
    ///         refused,
    ///         Err(TransportError::HostRefused(HostRefusal::Declined { .. }))
    ///     ));
    ///     assert_eq!(client.read_mixnet_indicator(), Indicator::Unattached);
    /// });
    /// ```
    pub async fn enable_mixnet_via_host(
        &mut self,
        host: impl crate::mixnet::acquire::ProxyHosting,
    ) -> Result<(), crate::mixnet::acquire::TransportError> {
        let indexer = self
            .connected_indexer_uri()
            .ok_or(crate::mixnet::MixnetProxyError::NoSessionIndexer)?;
        self.enable_mixnet_from(std::sync::Arc::new(
            crate::mixnet::acquire::Acquirer::Hosted(crate::mixnet::acquire::HostedAcquirer {
                provider: crate::mixnet::acquire::HostedProvider::owned_by(host),
                indexer,
            }),
        ))
        .await
    }

    /// Enables Mixnet Mode over `acquirer` by birthing the session's
    /// standing Proven Client — the same trust-or-probe birth every client
    /// gets — narrating on the session channel as the slot owner while the
    /// birth runs on its own channel, holding only the bound exit's lease,
    /// and leaving any failure `Unattached`.
    ///
    /// The enable is the user's consent to the mixnet from the moment it
    /// is asked for, so the transmit policy moves to
    /// [`TransmitPolicy::Mixnet`](crate::mixnet::TransmitPolicy) before the
    /// birth starts: a send during the bootstrap refuses as `Bootstrapping`
    /// rather than travelling a nakednet route the user has just turned
    /// away from. A failed enable restores the policy the user had before,
    /// so a session that chose nakednet keeps sending there after an
    /// attempt that did not take, and a session that never chose it keeps
    /// refusing.
    async fn enable_mixnet_from(
        &mut self,
        acquirer: std::sync::Arc<crate::mixnet::acquire::Acquirer>,
    ) -> Result<(), crate::mixnet::acquire::TransportError> {
        let policy_before = self.transmit_policy();
        self.set_transmit_policy(crate::mixnet::TransmitPolicy::Mixnet);
        self.vacate_mixnet_slot().await;
        // The slot owner alone speaks on the session channel: one
        // Bootstrapping for the whole enable, the settled state after it,
        // and never a candidate's churn.
        self.mixnet_status
            .send_replace(crate::mixnet::MixnetStatus {
                mode: crate::mixnet::Indicator::Bootstrapping,
                socks5_addr: None,
                exits: Vec::new(),
                bootstrap_detail: None,
                death: None,
            });
        // Boot proves three exits at once and assigns each a role in the
        // order it confirms (ADR 0045, narrowed by ADR 0063). The
        // IndexerClient's birth becomes the session's standing client; the
        // other two are held for the jobs boot gives them and stop when
        // those jobs end.
        match crate::mixnet::roles::prove_role_exits(&self.exit_pools, acquirer.as_ref()).await {
            Ok(role_exits) => {
                let birth = role_exits.indexer;
                // The two unspent conduits go to the exit authority, where
                // the job each role names takes its own without a birth.
                for (role, conduit) in [
                    (crate::mixnet::roles::Role::PriceFetch, role_exits.price),
                    (crate::mixnet::roles::Role::Spare, role_exits.spare),
                ] {
                    self.exit_pools.hold_conduit(role, conduit);
                }
                // The Standing Client's later transitions — above all
                // Died — must still reach the session's subscribers.
                forward_client_transitions(&birth.lifecycle, &self.mixnet_status);
                let client = standing_client_from_birth(birth);
                let superseded = swap_slot(
                    &self.mixnet_slot,
                    crate::mixnet::MixnetSlot::Attached(client),
                );
                debug_assert!(matches!(superseded, crate::mixnet::MixnetSlot::Unattached));
                self.exit_pools.set_acquirer(acquirer);
                self.arm_standing_watchdog();
                // The settled slot publishes last so subscribers read the
                // attached state whole.
                self.publish_mixnet_slot_state();
                Ok(())
            }
            Err(error) => {
                // A failed enable leaves Unattached rather than a prior
                // SwitchedOff, and hands the policy back to the user's
                // earlier choice; subscribers must see the settled state.
                self.set_transmit_policy(policy_before);
                self.publish_mixnet_slot_state();
                Err(error)
            }
        }
    }

    /// Records a completed round trip carried by the Standing Client,
    /// promoting stale proof to earned Ready and refreshing the exit's
    /// EpochProven observation.
    pub(crate) fn note_standing_round_trip(&self) {
        let promoted_node = {
            let guarded = self.mixnet_slot.lock().expect("mixnet slot mutex");
            match &*guarded {
                crate::mixnet::MixnetSlot::Attached(client) if client.note_round_trip() => {
                    Some(client.exit_node().cloned())
                }
                _ => None,
            }
        };
        if let Some(node) = promoted_node {
            if let Some(node) = node {
                self.exit_pools.remember(
                    node,
                    crate::mixnet::pools::exit_pool::ExitNodeHealthVerdict::EpochProven,
                );
            }
            // Subscribers watching the session channel see the promotion.
            self.publish_mixnet_slot_state();
        }
    }

    /// ```
    /// use zingolib::config::{ClientConfig, WalletConfig};
    /// use zingolib::lightclient::LightClient;
    /// use zingolib::mixnet::{ExitNodeId, Indicator, MixnetProxyError};
    ///
    /// tokio::runtime::Runtime::new().unwrap().block_on(async {
    ///     let wallet_dir = tempfile::tempdir().unwrap();
    ///     let config = ClientConfig::builder()
    ///         .set_wallet_dir(wallet_dir.path().to_path_buf())
    ///         .set_wallet_config(WalletConfig::MnemonicPhrase {
    ///             mnemonic_phrase: "abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon abandon abandon abandon \
    ///                 abandon abandon abandon abandon art"
    ///                 .to_string(),
    ///             no_of_accounts: std::num::NonZeroU32::new(1).unwrap(),
    ///             birthday: 2_000_000,
    ///             wallet_settings: Default::default(),
    ///         })
    ///         .build()
    ///         .unwrap();
    ///     let mut client = LightClient::new(config, true).await.unwrap();
    ///
    ///     // An Indexerless session has nothing for the readiness round
    ///     // trip to reach, so the attach refuses before it parses anything
    ///     // (zingo-adrs zingolib/0063).
    ///     let indexerless = client.attach_mixnet("127.0.0.1:9", &[]).await;
    ///     assert!(matches!(indexerless, Err(MixnetProxyError::NoSessionIndexer)));
    ///     client
    ///         .set_indexer_uri_lazy("https://indexer.test:443".parse().unwrap())
    ///         .unwrap();
    ///
    ///     // An unparseable address refuses typed, leaving Unattached.
    ///     let refused = client.attach_mixnet("not-an-address", &[]).await;
    ///     assert!(matches!(
    ///         refused,
    ///         Err(MixnetProxyError::InvalidAddress { .. })
    ///     ));
    ///     assert_eq!(client.read_mixnet_indicator(), Indicator::Unattached);
    ///
    ///     // Empty `exits` refuses typed before anything is dialed.
    ///     let exitless = client.attach_mixnet("127.0.0.1:9", &[]).await;
    ///     assert!(matches!(exitless, Err(MixnetProxyError::NoExits)));
    ///
    ///     // A well-formed endpoint installs as the Standing Client, and
    ///     // readiness is validated asynchronously: the call returns before
    ///     // any probe, so the mode is not yet Ready.
    ///     let host_drawn = ExitNodeId::parse("host-drawn-exit").unwrap();
    ///     client
    ///         .attach_mixnet("127.0.0.1:9", std::slice::from_ref(&host_drawn))
    ///         .await
    ///         .unwrap();
    ///     assert_ne!(client.read_mixnet_indicator(), Indicator::Ready);
    ///
    ///     // A later attach vacates the standing transport first, so its
    ///     // failure leaves Unattached rather than the prior client.
    ///     client.attach_mixnet("not-an-address", &[]).await.unwrap_err();
    ///     assert_eq!(client.read_mixnet_indicator(), Indicator::Unattached);
    /// });
    /// ```
    ///
    /// Like every enable, the attach is the user's consent to the mixnet
    /// from the moment it is asked for: the transmit policy moves to
    /// [`TransmitPolicy::Mixnet`](crate::mixnet::TransmitPolicy) before the
    /// attach, so a send while the readiness gate runs refuses as
    /// `Bootstrapping` rather than leaking. A refused attach restores the
    /// policy the user had before.
    pub async fn attach_mixnet(
        &mut self,
        socks5_addr: &str,
        exits: &[crate::mixnet::ExitNodeId],
    ) -> Result<(), crate::mixnet::MixnetProxyError> {
        let indexer = self
            .connected_indexer_uri()
            .ok_or(crate::mixnet::MixnetProxyError::NoSessionIndexer)?;
        let policy_before = self.transmit_policy();
        self.set_transmit_policy(crate::mixnet::TransmitPolicy::Mixnet);
        self.vacate_mixnet_slot().await;
        let attached = socks5_addr
            .parse()
            .map_err(|_| crate::mixnet::MixnetProxyError::InvalidAddress {
                addr: socks5_addr.to_string(),
            })
            .and_then(|socks5_addr| {
                crate::mixnet::MixnetProxy::attach(
                    socks5_addr,
                    exits,
                    indexer,
                    std::sync::Arc::clone(&self.mixnet_status),
                )
            });
        match attached {
            Ok(proxy) => {
                // The mobile host drew the attached endpoint's exit outside
                // this session's Exit Pool, so the Standing Client holds no
                // exit_reservation.
                // The attach readiness gate earns the proof end to end, so
                // the hosted Standing Client is born probed.
                let superseded = swap_slot(
                    &self.mixnet_slot,
                    crate::mixnet::MixnetSlot::Attached(crate::mixnet::StandingClient::new(
                        proxy, None, true,
                    )),
                );
                debug_assert!(matches!(superseded, crate::mixnet::MixnetSlot::Unattached));
                self.arm_standing_watchdog();
                Ok(())
            }
            Err(error) => {
                // A refused attach leaves Unattached rather than a prior
                // SwitchedOff, and hands the policy back to the user's
                // earlier choice; subscribers must see the settled state.
                self.set_transmit_policy(policy_before);
                self.publish_mixnet_slot_state();
                Err(error)
            }
        }
    }

    /// Disable Mixnet Mode — the deliberate, per-session choice that alone
    /// reaches [`Indicator::SwitchedOff`](crate::mixnet::Indicator) — shutting
    /// down any running transport. A transport act only: the transmit
    /// policy is untouched, so where a send travels afterwards is the
    /// policy's answer, and the mixnet-only surfaces refuse.
    pub async fn disable_mixnet(&mut self) {
        self.vacate_mixnet_slot().await;
        swap_slot(&self.mixnet_slot, crate::mixnet::MixnetSlot::SwitchedOff);
        self.publish_mixnet_slot_state();
    }

    /// Publish the slot's current state into the session status channel,
    /// called only after a slot transition settles — never mid-replacement —
    /// so subscribers see deliberate states and not the transient unattached
    /// between a vacate and its successor.
    pub(super) fn publish_mixnet_slot_state(&self) {
        publish_slot(&self.mixnet_slot, &self.mixnet_status);
    }

    /// - Under `OptedOutThisSession`, vacates the transport slot and sets
    ///   the session's transmit policy to nakednet.
    /// - Under `ForcedOn` with `Spawn`, resolves the `nym-proxy` path from
    ///   the hints, spawns the child, and blocks until the standing client
    ///   is born proven.
    /// - Under `ForcedOn` with `Attach`, attaches to the host's SOCKS5
    ///   endpoint.
    /// - Publishes the settled slot state to the session status channel.
    pub async fn start_mixnet_session(
        &mut self,
        strategy: crate::mixnet::ProvisionStrategy<'_>,
        policy: crate::mixnet::MixnetStartPolicy,
    ) -> Result<crate::mixnet::SessionStarted, crate::mixnet::SessionStartError> {
        use crate::mixnet;
        match policy {
            mixnet::MixnetStartPolicy::OptedOutThisSession => {
                self.disable_mixnet().await;
                // The opt-out is the user's explicit nakednet consent for
                // this session, so it is the one production act that sets
                // the policy. The price fetch stays mixnet-only regardless.
                self.set_transmit_policy(mixnet::TransmitPolicy::Nakednet);
                Ok(mixnet::SessionStarted::OptedOut)
            }
            mixnet::MixnetStartPolicy::ForcedOn => match strategy {
                mixnet::ProvisionStrategy::Spawn(hints) => {
                    let resolution = mixnet::provision::resolve_proxy_path(&hints);
                    log::info!("mixnet session start: spawning nym-proxy at {resolution}");
                    // The go-online moment is a user act: someone is waiting.
                    match self
                        .enable_mixnet(std::path::Path::new(&resolution.path))
                        .await
                    {
                        Ok(()) => Ok(mixnet::SessionStarted::Spawned { resolution }),
                        Err(source) => Err(mixnet::SessionStartError::Spawn { resolution, source }),
                    }
                }
                mixnet::ProvisionStrategy::Attach { socks5_addr, exits } => self
                    .attach_mixnet(socks5_addr, exits)
                    .await
                    .map(|()| mixnet::SessionStarted::Attached {
                        socks5_addr: socks5_addr.to_string(),
                    })
                    .map_err(|source| mixnet::SessionStartError::Attach {
                        socks5_addr: socks5_addr.to_string(),
                        source,
                    }),
            },
        }
    }

    /// Subscribe to Mixnet Mode — the receiving half of the session's one
    /// status channel — whose keep-only-latest push delivers a typed
    /// [`MixnetStatus`](crate::mixnet::MixnetStatus) snapshot on every
    /// transition, in a receiver independent of this client borrow that
    /// survives enable/disable cycles.
    pub fn subscribe_mixnet_status(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::mixnet::MixnetStatus> {
        self.mixnet_status.subscribe()
    }

    pub fn read_mixnet_indicator(&self) -> crate::mixnet::Indicator {
        self.mixnet_slot.lock().expect("mixnet slot mutex").mode()
    }

    /// The local SOCKS5 address while Mixnet Mode is ready.
    pub fn mixnet_socks5_addr(&self) -> Option<std::net::SocketAddr> {
        self.mixnet_slot
            .lock()
            .expect("mixnet slot mutex")
            .socks5_addr()
    }

    /// The session's conduit while a transport is attached.
    pub fn mixnet_conduit(&self) -> Option<crate::mixnet::MixnetConduit> {
        self.mixnet_slot
            .lock()
            .expect("mixnet slot mutex")
            .conduit()
    }

    /// Switch Mixnet Mode on for a chain-mock test, with the slot reporting
    /// [`Indicator::Ready`](crate::mixnet::Indicator) at `socks5_addr` while
    /// no child, watcher, or probe stands behind it, so the test walks the
    /// same fail-closed route resolver and retry policy a live
    /// Ready session does and the transmit path submits over the mock
    /// indexer's channel without ever dialing the address.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn switch_on_mixnet_for_tests(&mut self, socks5_addr: std::net::SocketAddr) {
        self.attach_mixnet_for_tests(socks5_addr, true).await;
    }

    /// Switch Mixnet Mode on over the SOCKS5 relay at `socks5_addr`, with real mixnet arms.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn switch_on_mixnet_through_for_tests(&mut self, socks5_addr: std::net::SocketAddr) {
        self.attach_mixnet_for_tests(socks5_addr, false).await;
    }

    #[cfg(any(test, feature = "testutils"))]
    async fn attach_mixnet_for_tests(
        &mut self,
        socks5_addr: std::net::SocketAddr,
        mock_arms: bool,
    ) {
        self.vacate_mixnet_slot().await;
        swap_slot(
            &self.mixnet_slot,
            crate::mixnet::MixnetSlot::AttachedForTests {
                socks5_addr,
                conduit: crate::mixnet::MixnetConduit::over(socks5_addr),
                mock_arms,
            },
        );
        // Every slot transition publishes (the one-shared-watch invariant),
        // the stand-in included.
        self.publish_mixnet_slot_state();
        // The stand-in carries the enable's consent act.
        self.set_transmit_policy(crate::mixnet::TransmitPolicy::Mixnet);
    }

    /// The proxy's latest bootstrap progress line while Mixnet Mode is
    /// bootstrapping, so a user interface can narrate the connect race.
    pub fn mixnet_bootstrap_detail(&self) -> Option<String> {
        self.mixnet_slot
            .lock()
            .expect("mixnet slot mutex")
            .proxy()
            .and_then(|proxy| proxy.bootstrap_detail())
    }

    /// Why the transport died, while Mixnet Mode is
    /// [`Indicator::Died`](crate::mixnet::Indicator) and the watcher held a
    /// typed cause: the [`zl_netutils::diag::NetOpFailure`] record naming the
    /// stage, the target, and the cause chain as a vector, so a `died`
    /// verdict carries *why* without anyone parsing prose.
    pub fn mixnet_death_detail(&self) -> Option<zl_netutils::diag::NetOpFailure> {
        self.mixnet_slot
            .lock()
            .expect("mixnet slot mutex")
            .proxy()
            .and_then(|proxy| proxy.death_detail())
    }

    /// The latched death read whole — its moment and, when the watcher held
    /// one, its typed cause — while Mixnet Mode is
    /// [`Indicator::Died`](crate::mixnet::Indicator) and `None` in every
    /// other mode, the moment distinguishing a stale latch from a fresh one
    /// through [`crate::mixnet::DeathReport::age`].
    pub fn mixnet_death_report(&self) -> Option<crate::mixnet::DeathReport> {
        self.mixnet_slot
            .lock()
            .expect("mixnet slot mutex")
            .proxy()
            .and_then(|proxy| proxy.death_report())
    }

    /// Resolve the route of a mixnet-only surface (price-fetch, the liveness
    /// probe): the session's conduit while Mixnet Mode is
    /// [`Indicator::Ready`](crate::mixnet::Indicator::Ready), a typed
    /// refusal in every other state. The transmit policy has no say here.
    pub fn mixnet_only_route(
        &self,
    ) -> Result<crate::mixnet::MixnetConduit, crate::mixnet::MixnetNotReady> {
        crate::mixnet::resolve_mixnet_only_route(
            self.read_mixnet_indicator(),
            self.mixnet_conduit(),
        )
    }

    /// Resolve the route of a transmission under the session's
    /// [`TransmitPolicy`](crate::mixnet::TransmitPolicy): nakednet at once
    /// under the nakednet policy, and under the mixnet policy the conduit
    /// or the refusal [`Self::mixnet_only_route`] would give.
    pub fn send_route(&self) -> Result<crate::mixnet::MixnetRoute, crate::mixnet::MixnetNotReady> {
        crate::mixnet::resolve_send_route(
            self.transmit_policy(),
            self.read_mixnet_indicator(),
            self.mixnet_conduit(),
        )
    }

    /// The session's transmit policy, readable so a consumer can show the
    /// current mode.
    pub fn transmit_policy(&self) -> crate::mixnet::TransmitPolicy {
        if self
            .transmit_over_mixnet
            .load(std::sync::atomic::Ordering::Acquire)
        {
            crate::mixnet::TransmitPolicy::Mixnet
        } else {
            crate::mixnet::TransmitPolicy::Nakednet
        }
    }

    /// Set the session's transmit policy. Takes effect on the next route
    /// resolution, so a transmission already in flight keeps the route it
    /// resolved. Never persisted: every session starts under
    /// [`TransmitPolicy::Mixnet`](crate::mixnet::TransmitPolicy).
    pub fn set_transmit_policy(&self, policy: crate::mixnet::TransmitPolicy) {
        self.transmit_over_mixnet.store(
            policy == crate::mixnet::TransmitPolicy::Mixnet,
            std::sync::atomic::Ordering::Release,
        );
    }

    /// Runs the mixnet liveness probe — concurrent `GetLightdInfo` calls
    /// through the session's SOCKS5 proxy, each appending its outcome to the
    /// cross-session indexer history — against `target`, or against the
    /// Session Indexer when `target` is `None`, with no nakednet leg and a
    /// refusal while the mixnet transport is not ready.
    pub async fn probe_destinations(
        &self,
        target: Option<http::Uri>,
        timeout: std::time::Duration,
    ) -> Result<Vec<crate::mixnet::probe::MixnetProbe>, crate::lightclient::error::LightClientError>
    {
        // The guard lives for the whole probe, so the conduit counts this
        // work as outstanding until every leg has finished.
        let dial = self.mixnet_only_route()?.dial();
        let socks5_addr = dial.socks5();
        // An Indexerless session has nothing to probe, and an indexer outside
        // the exit policy's endpoint shape is refused whether the caller
        // named it or the session holds it, never answered with silence.
        let indexer = target
            .or_else(|| self.connected_indexer_uri())
            .ok_or(crate::lightclient::error::LightClientError::Offline)?;
        if !crate::mixnet::probe::probe_eligible(&indexer) {
            return Err(
                crate::lightclient::error::LightClientError::IneligibleProbeTarget(indexer),
            );
        }
        let probe = crate::mixnet::probe::probe_indexer(
            &indexer,
            socks5_addr,
            timeout,
            &self.indexer_history,
        )
        .await;
        // An answered probe is a completed round trip through the Standing
        // Client, promoting stale proof to earned; an unanswered one raises
        // the suspicion the standing exit is dead, and the arbiter probe
        // adjudicates.
        if probe.leg.outcome.is_ok() {
            self.note_standing_round_trip();
        } else {
            self.note_standing_exit_suspicion();
        }
        Ok(vec![probe])
    }

    /// Update and return the current ZEC price in USD by racing the price
    /// sources through the tunnel and taking the first answer. Mixnet-only
    /// (ADR 0011, amendment 2026-09-11): a ready mixnet carries it, every
    /// other state refuses with a typed
    /// [`MixnetNotReady`](crate::mixnet::MixnetNotReady), and the transmit
    /// policy has no effect.
    pub async fn update_current_price(&self) -> Result<MixnetPriceFetch, LightClientError> {
        let dispatched = std::time::Instant::now();
        let conduit = self.mixnet_only_route()?;
        let dial = conduit.dial();
        let socks5_addr = dial.socks5();

        let run = PriceRun {
            pools: self.exit_pools.clone(),
        };

        let (outcomes, via_socks5) = if self.exit_pools.acquirer().is_some() {
            let (outcomes, spent) = crate::mixnet::speed::run_speed_prioritized(&run)
                .await
                .map_err(crate::wallet::error::PriceError::Speed)?;
            let dial = spent
                .addr()
                .map(|addr| addr.to_string())
                .unwrap_or_default();
            crate::mixnet::speed::SpeedPrioritized::dispose(&run, spent);
            (outcomes, dial)
        } else {
            let conduit = zl_netutils::conduit::MixnetConduit::over(socks5_addr);
            match crate::mixnet::speed::run_wave(&run, &conduit).await {
                crate::mixnet::speed::WaveEnd::Settled(outcomes)
                | crate::mixnet::speed::WaveEnd::Exhausted(outcomes) => {
                    (outcomes, socks5_addr.to_string())
                }
            }
        };
        let raced =
            crate::price::first_quote(outcomes).map_err(crate::wallet::error::PriceError::from)?;
        Ok(MixnetPriceFetch {
            usd: raced.price.price_usd,
            source: raced.source,
            round_trip: dispatched.elapsed(),
            route: crate::lightclient::PriceFetchRoute::Mixnet { via_socks5 },
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod proof_acquisition;

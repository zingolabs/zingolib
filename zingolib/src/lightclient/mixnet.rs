//! Mixnet Mode toggle (ADR 0011, consumption model A). Enabling spawns the
//! bundled `nym-proxy` child process. Disabling shuts it down. The mode
//! reflects the transport slot's state, and clearnet is reachable only by a
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
    pools: std::sync::Arc<crate::correspondent::pool::Pools>,
}

#[cfg(feature = "nym")]
impl crate::mixnet::speed::SpeedPrioritized for PriceRun {
    type Target = zingo_price::PriceSource;
    type Outcome = (
        zingo_price::PriceSource,
        Result<zingo_price::Price, zingo_price::PriceError>,
    );

    fn targets(&self) -> Vec<Self::Target> {
        zingo_price::RACED_SOURCES.to_vec()
    }

    async fn probe(
        &self,
        dial: zingo_netutils::conduit::ConduitDial,
        target: Self::Target,
    ) -> Self::Outcome {
        let outcome = zingo_price::get_source_price(
            target,
            &dial,
            target.url(),
            zingo_price::REQUEST_TIMEOUT,
            zingo_price::CONNECT_TIMEOUT,
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
                .conduit_for(crate::mixnet::quartet::Role::PriceFetch, acquirer.as_ref())
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
            crate::correspondent::pool::exit_pool::ExitNodeHealthVerdict::EpochProven,
        );
        tokio::spawn(async move {
            spent.retire().await;
        });
    }

    fn abandon(&self, dead: crate::mixnet::speed::Member) {
        self.pools.remember(
            dead.node().clone(),
            crate::correspondent::pool::exit_pool::ExitNodeHealthVerdict::Failed,
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
    birth: crate::correspondent::pool::ProvenBirth,
) -> crate::mixnet::StandingClient {
    use crate::correspondent::pool::Proof;

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
    pools: std::sync::Arc<crate::correspondent::pool::Pools>,
    status: crate::mixnet::driver::StatusPublisher,
    evidence: zingo_netutils::sentinel::ExitEvidence,
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
                    crate::correspondent::pool::exit_pool::ExitNodeHealthVerdict::EpochProven,
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
            crate::correspondent::pool::exit_pool::ExitNodeHealthVerdict::Failed,
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
            client.forsake(zingo_net_diag::NetOpFailure::message(
                zingo_net_diag::NetOpStage::RouteResolution,
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
                client.forsake(zingo_net_diag::NetOpFailure::from_error(
                    zingo_net_diag::NetOpStage::RouteResolution,
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
    pools: std::sync::Arc<crate::correspondent::pool::Pools>,
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
        zingo_netutils::sentinel::probe_sentinel(socks5, zingo_netutils::time::SENTINEL_BUDGET)
            .await;
    adjudicate_standing_proof(slot, pools, status, evidence).await;
}

/// The expiry watchdog: sleeps until the Standing Client's proof deadline,
/// fires a new ProofAcquisition at that earliest feasible moment, follows
/// replacement clients in the slot until the slot empties, and parks on the
/// status channel whenever an acquisition leaves a lapsed deadline
/// unadvanced.
async fn standing_proof_watchdog(
    slot: std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    pools: std::sync::Arc<crate::correspondent::pool::Pools>,
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
    pools: &std::sync::Arc<crate::correspondent::pool::Pools>,
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
    pools: std::sync::Arc<crate::correspondent::pool::Pools>,
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
            let mut entropy = rand::thread_rng();
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
    /// `Unattached` that a failed enable also deliberately leaves behind,
    /// because the enable act revoked any standing clearnet consent and a
    /// failure must not silently reinstate a prior `SwitchedOff`.
    pub(super) async fn vacate_mixnet_slot(&mut self) {
        self.correspondent_pools.clear_acquirer();
        // Boot's unspent conduits go with the session: a teardown before
        // their jobs took them must leave no proxy behind.
        for conduit in self.correspondent_pools.drain_conduits() {
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
            self.correspondent_pools.clone(),
            std::sync::Arc::clone(&self.mixnet_status),
        )));
        // Both watchdogs follow the slot rather than a client, so a rotation
        // or a failover installing a replacement re-arms neither.
        self.rotation_watchdog = Some(tokio::spawn(standing_rotation_watchdog(
            self.mixnet_slot.clone(),
            self.correspondent_pools.clone(),
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
            self.correspondent_pools.clone(),
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
    ///     // enable leaves Unattached — a refusal, never clearnet.
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
    /// use zingolib::mixnet::{ExitNodeId, Indicator, TransportError};
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
    ///     // The standing client births from the host instead of a spawned
    ///     // binary; the host's refusal surfaces typed and the failed
    ///     // enable leaves Unattached — a refusal, never clearnet.
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
        self.enable_mixnet_from(std::sync::Arc::new(
            crate::mixnet::acquire::Acquirer::Hosted(
                crate::mixnet::acquire::HostedProvider::owned_by(host),
            ),
        ))
        .await
    }

    /// Enables Mixnet Mode over `acquirer` by birthing the session's
    /// standing Proven Client — the same trust-or-probe birth every client
    /// gets — narrating on the session channel as the slot owner while the
    /// birth runs on its own channel, holding only the bound exit's lease,
    /// and leaving any failure `Unattached`.
    async fn enable_mixnet_from(
        &mut self,
        acquirer: std::sync::Arc<crate::mixnet::acquire::Acquirer>,
    ) -> Result<(), crate::mixnet::acquire::TransportError> {
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
        // Boot proves four exits at once and assigns each a role in the
        // order it confirms (ADR 0045). The IndexerClient's birth becomes
        // the session's standing client; the other three are held for the
        // jobs boot gives them and stop when those jobs end.
        match crate::mixnet::quartet::prove_quartet(&self.correspondent_pools, acquirer.as_ref())
            .await
        {
            Ok(quartet) => {
                let birth = quartet.indexer;
                // The three unspent conduits go to the exit authority, where
                // the job each role names takes its own without a birth.
                for (role, conduit) in [
                    (crate::mixnet::quartet::Role::IndexerSweep, quartet.sweep),
                    (crate::mixnet::quartet::Role::PriceFetch, quartet.price),
                    (crate::mixnet::quartet::Role::Spare, quartet.spare),
                ] {
                    self.correspondent_pools.hold_conduit(role, conduit);
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
                self.correspondent_pools.set_acquirer(acquirer);
                self.arm_standing_watchdog();
                // The settled slot publishes last so subscribers read the
                // attached state whole.
                self.publish_mixnet_slot_state();
                Ok(())
            }
            Err(error) => {
                // A failed enable leaves Unattached (the user's enable revoked
                // any standing clearnet consent); subscribers must see it.
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
                self.correspondent_pools.remember(
                    node,
                    crate::correspondent::pool::exit_pool::ExitNodeHealthVerdict::EpochProven,
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
    pub async fn attach_mixnet(
        &mut self,
        socks5_addr: &str,
        exits: &[crate::mixnet::ExitNodeId],
    ) -> Result<(), crate::mixnet::MixnetProxyError> {
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
                // A failed enable leaves Unattached (the enable act revoked
                // any standing clearnet consent); subscribers must see it.
                self.publish_mixnet_slot_state();
                Err(error)
            }
        }
    }

    /// Disable Mixnet Mode — the deliberate, per-session choice that alone
    /// reaches [`Indicator::SwitchedOff`](crate::mixnet::Indicator) — shutting
    /// down any running transport so the mixnet-only surfaces route over
    /// clearnet as informed consent.
    pub async fn disable_mixnet(&mut self) {
        // Revoked consent stops every networking act this session started:
        // the health sweep's survey halts with the standing transport.
        self.abort_health_sweep().await;
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

    /// The driver entry a session calls at its go-online moment, which under
    /// [`MixnetStartPolicy::ForcedOn`](crate::mixnet::MixnetStartPolicy)
    /// provisions the transport by `strategy` (the bundled binary spawned
    /// from the consumer's platform hints, or an attach to a mobile-platform-hosted
    /// endpoint) and blocks until the standing client is born proven, under
    /// [`MixnetStartPolicy::OptedOutThisSession`](crate::mixnet::MixnetStartPolicy)
    /// records the startup opt-out as the explicit act that reaches switched
    /// off, returns any provisioning failure typed while leaving the mode
    /// unattached — refusal, never a silent clearnet — and never respawns on
    /// its own, recovery staying explicit through
    /// [`Indicator::needs_recovery`](crate::mixnet::Indicator::needs_recovery).
    pub async fn start_mixnet_session(
        &mut self,
        strategy: crate::mixnet::ProvisionStrategy<'_>,
        policy: crate::mixnet::MixnetStartPolicy,
    ) -> Result<(), crate::mixnet::acquire::TransportError> {
        match policy {
            crate::mixnet::MixnetStartPolicy::OptedOutThisSession => {
                self.disable_mixnet().await;
                Ok(())
            }
            crate::mixnet::MixnetStartPolicy::ForcedOn => match strategy {
                crate::mixnet::ProvisionStrategy::Spawn(hints) => {
                    let path = crate::mixnet::provision::resolve_proxy_path(&hints);
                    log::info!("mixnet session start: spawning nym-proxy at {path}");
                    // The go-online moment is a user act: someone is waiting.
                    self.enable_mixnet(std::path::Path::new(&path)).await
                }
                crate::mixnet::ProvisionStrategy::Attach { socks5_addr, exits } => self
                    .attach_mixnet(socks5_addr, exits)
                    .await
                    .map_err(crate::mixnet::acquire::TransportError::from),
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
    /// same fail-closed route resolver and escalation orchestration a live
    /// Ready session does and the transmit path submits over the mock
    /// indexer's channel without ever dialing the address.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn switch_on_mixnet_for_tests(&mut self, socks5_addr: std::net::SocketAddr) {
        self.vacate_mixnet_slot().await;
        swap_slot(
            &self.mixnet_slot,
            crate::mixnet::MixnetSlot::AttachedForTests {
                socks5_addr,
                conduit: crate::mixnet::MixnetConduit::over(socks5_addr),
            },
        );
        // Every slot transition publishes (the one-shared-watch invariant),
        // the stand-in included.
        self.publish_mixnet_slot_state();
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
    /// typed cause: the [`zingo_net_diag::NetOpFailure`] record naming the
    /// stage, the target, and the cause chain as a vector, so a `died`
    /// verdict carries *why* without anyone parsing prose.
    pub fn mixnet_death_detail(&self) -> Option<zingo_net_diag::NetOpFailure> {
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

    /// Resolve the fail-closed route every mixnet-only surface must obey —
    /// the mixnet proxy when
    /// [`Indicator::Ready`](crate::mixnet::Indicator::Ready), clearnet only
    /// when switched off (the deliberate toggle-off), and a refusal while
    /// unattached, bootstrapping, or died — as the single resolver that
    /// send, price-fetch, and the liveness probe share.
    pub fn mixnet_route(
        &self,
    ) -> Result<crate::mixnet::MixnetRoute, crate::mixnet::MixnetNotReady> {
        crate::mixnet::resolve_route(self.read_mixnet_indicator(), self.mixnet_conduit())
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
    ) -> Result<Vec<crate::mixnet::probe::MixnetProbe>, crate::lightclient::error::LightClientError>
    {
        // The guard lives for the whole probe, so the conduit counts this
        // work as outstanding until every leg has finished.
        let dial = match self.mixnet_route()? {
            crate::mixnet::MixnetRoute::Mixnet(conduit) => conduit.dial(),
            crate::mixnet::MixnetRoute::Clearnet => {
                return Err(crate::lightclient::error::LightClientError::ProbeRequiresMixnet);
            }
        };
        let socks5_addr = dial.socks5();
        if let Some(uri) = &target
            && !crate::mixnet::probe::probe_eligible(uri)
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
            .filter(crate::mixnet::probe::probe_eligible)
            .collect();
        let history = self.indexer_history.clone();
        let probes = futures::future::join_all(targets.iter().map(|indexer| {
            crate::mixnet::probe::probe_indexer(indexer, socks5_addr, timeout, &history)
        }))
        .await;
        // Any answered probe is a completed round trip through the Standing
        // Client, promoting stale proof to earned; a wave nobody answered
        // raises the suspicion the standing exit is dead, and the arbiter
        // probe adjudicates.
        if probes.iter().any(|probe| probe.leg.outcome.is_ok()) {
            self.note_standing_round_trip();
        } else if !probes.is_empty() {
            self.note_standing_exit_suspicion();
        }
        Ok(probes)
    }

    /// Update and return the current ZEC price in USD by racing the three
    /// price sources (Gemini, Kraken, CoinGecko) through the mixnet tunnel —
    /// hiding the client IP, taking the first answer, erroring only when
    /// every source fails and then naming each source's typed failure, and
    /// returning the tunnel endpoint, the winning source, and the round-trip
    /// time as per-fetch route evidence — while failing closed in every
    /// other state: a typed [`MixnetNotReady`](crate::mixnet::MixnetNotReady)
    /// refusal while unattached, bootstrapping, or died, and
    /// [`LightClientError::PriceFetchRequiresMixnet`] while switched off,
    /// because the switched-off consent covers Transmission and never a
    /// third-party price API outside the Zcash ecosystem.
    pub async fn update_current_price(&self) -> Result<MixnetPriceFetch, LightClientError> {
        // Held for the whole race, so the conduit knows the fetch is still
        // running even while every source is in flight.
        let dial = match self.mixnet_route()? {
            crate::mixnet::MixnetRoute::Mixnet(conduit) => conduit.dial(),
            crate::mixnet::MixnetRoute::Clearnet => {
                return Err(LightClientError::PriceFetchRequiresMixnet);
            }
        };
        let socks5_addr = dial.socks5();

        // A spawned session births a per-run Proven Client — one fresh
        // Shared exit per run, never the standing client's — while an
        // attached session's single mobile-platform endpoint carries it as
        // before. The per-run client is stopped whatever the outcome, and
        // its lease recycles into the Exit Pool.
        // The run is the one speed-priority operation shape: it acquires a
        // transport, races its sources through that exit in a wave a
        // Sentinel rides, and redraws whenever an exit proves to carry
        // nothing. Acquisition, disposal, and the redraw bound are the
        // shared loop's; this call site owns only the quote.
        let dispatched = std::time::Instant::now();
        let run = PriceRun {
            pools: self.correspondent_pools.clone(),
        };
        // A spawned session draws its own transport per run, so a dead exit
        // costs a redraw. An attached session has one endpoint its platform
        // host owns: there is nothing to redraw, so the run rides that tunnel
        // for one wave, and a dead exit ends it.
        let (outcomes, via_socks5) = if self.correspondent_pools.acquirer().is_some() {
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
            let conduit = zingo_netutils::conduit::MixnetConduit::over(socks5_addr);
            match crate::mixnet::speed::run_wave(&run, &conduit).await {
                crate::mixnet::speed::WaveEnd::Settled(outcomes)
                | crate::mixnet::speed::WaveEnd::Exhausted(outcomes) => {
                    (outcomes, socks5_addr.to_string())
                }
            }
        };
        let raced =
            zingo_price::first_quote(outcomes).map_err(crate::wallet::error::PriceError::from)?;
        let round_trip = dispatched.elapsed();
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
        //! Bootstrapping, Died}`) pair with `mixnet::route`'s own
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
                    LightClientError::MixnetNotReady(crate::mixnet::MixnetNotReady::Unattached)
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
        /// consents to clearnet.
        #[tokio::test]
        async fn disable_before_any_enable_records_clearnet_consent() {
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
            assert!(matches!(
                client.mixnet_route(),
                Ok(crate::mixnet::MixnetRoute::Clearnet)
            ));
        }
    }

    mod consent_holds {
        //! Revoked consent stops every networking act the session started
        //! (findings 2 and 9 of the 2026-08-14 PR #2695 review).
        use crate::lightclient::LightClient;
        use crate::mixnet::ExitNodeId;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        fn wallet() -> crate::wallet::LightWallet {
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build()
        }

        /// A Standing Client over an attach to `addr`, for slot tests.
        fn attached_client(addr: std::net::SocketAddr) -> crate::mixnet::StandingClient {
            let proxy = crate::mixnet::MixnetProxy::attach(
                addr,
                &[ExitNodeId::from("exit-alpha")],
                crate::mixnet::status_publisher(),
            )
            .expect("the attach accepts a bound exit");
            crate::mixnet::StandingClient::new(proxy, None, true)
        }

        /// HYPOTHESIS: disabling the mixnet halts the health sweep, so the
        /// consent revocation stops the census probing the session started.
        /// Falsified if the sweep's task survives the disable.
        #[tokio::test]
        async fn disable_mixnet_halts_the_health_sweep() {
            let mut client = LightClient::new_for_test(wallet()).await;
            let (held_tx, cancelled_rx) = tokio::sync::oneshot::channel::<()>();
            let sweep = tokio::spawn(async move {
                let _held = held_tx;
                std::future::pending::<()>().await
            });
            *client
                .health_sweep
                .lock()
                .expect("the health-sweep slot is never poisoned") = Some(sweep);

            client.disable_mixnet().await;

            assert!(
                client
                    .health_sweep
                    .lock()
                    .expect("the health-sweep slot is never poisoned")
                    .is_none(),
                "the disable takes and halts the sweep"
            );
            assert!(
                cancelled_rx.await.is_err(),
                "the sweep task was cancelled, not left surveying"
            );
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
            let slot = std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Attached(old),
            ));

            let Ok(superseded) =
                super::super::install_failover_client(&slot, attached_client(addr))
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
            let slot = std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Attached(attached_client(addr)),
            ));

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
            let Err(refused) =
                super::super::install_failover_client(&healthy, attached_client(addr))
            else {
                panic!("a healthy incumbent has no failover to continue");
            };
            refused.stop().await;

            let condemned_client = attached_client(addr);
            condemned_client.condemn();
            let condemned = std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Attached(condemned_client),
            ));
            let Err(refused) =
                super::super::install_rotated_client(&condemned, attached_client(addr))
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
            let slot = std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Attached(attached_client(addr)),
            ));
            let pools = crate::correspondent::pool::Pools::new();
            // The desktop acquirer is the production Never, and its verdict
            // is answered without the binary ever being spawned.
            pools.set_acquirer(std::sync::Arc::new(
                crate::mixnet::acquire::Acquirer::Spawned(
                    crate::mixnet::acquire::SpawnedBinary::at("nym-proxy".into()),
                ),
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
            ) -> Result<crate::mixnet::MixnetProxy, crate::mixnet::acquire::TransportError>
            {
                Err(crate::mixnet::acquire::TransportError::NotReady {
                    budget: zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT,
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
            let slot = std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Attached(attached_client(addr)),
            ));
            let pools = crate::correspondent::pool::Pools::new();
            let status = crate::mixnet::status_publisher();
            let mut watching = status.subscribe();
            watching.borrow_and_update();

            super::super::hand_off_standing_client(&slot, &pools, &status, &BirthlessAcquirer)
                .await;

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
            let slot = std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Attached(client),
            ));
            let pools = crate::correspondent::pool::Pools::new();
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
        /// quartet assigns.
        const HARNESS_EXIT: &str = "exit-alpha";

        /// Every exit this harness discovers. Boot proves a quartet, so a
        /// population smaller than [`quartet::QUARTET_SIZE`] cannot fill
        /// the roles and the enable refuses.
        const HARNESS_CENSUS: [&str; crate::mixnet::quartet::QUARTET_SIZE] =
            [HARNESS_EXIT, "exit-beta", "exit-gamma", "exit-delta"];

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
                client.correspondent_pools.remember(
                    ExitNodeId::from(exit),
                    crate::correspondent::pool::exit_pool::ExitNodeHealthVerdict::EpochProven,
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
                birth_channel,
            )
            .expect("the attach accepts a bound exit");
            let client = crate::mixnet::StandingClient::new(proxy, None, true);
            let exhausted = crate::mixnet::acquire::TransportError::NoProvenExit {
                probed: crate::correspondent::pool::MAX_PROVING_BIRTHS,
                budget: zingo_netutils::time::SENTINEL_BUDGET,
            };
            client.forsake(zingo_net_diag::NetOpFailure::from_error(
                zingo_net_diag::NetOpStage::RouteResolution,
                "the session's exit census",
                &exhausted,
            ));
            let slot = std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Attached(client),
            ));

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

        /// HYPOTHESIS: an enable act revokes standing clearnet consent even
        /// when the mobile platform address fails to parse — from `SwitchedOff`,
        /// a failed `attach_mixnet` lands `Unattached` and publishes it.
        /// Falsified if the mode remains `SwitchedOff` after the failed
        /// attach.
        #[tokio::test]
        async fn a_failed_attach_revokes_clearnet_consent() {
            let mut client = LightClient::new_for_test(wallet()).await;
            client.disable_mixnet().await;
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
                "subscribers must see the revocation"
            );
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
                crate::mixnet::Indicator::Unattached,
                "the channel opens in the ground state"
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
                crate::mixnet::acquire::TransportError::Proxy(
                    crate::mixnet::MixnetProxyError::InvalidAddress { .. }
                )
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
            let mut client = LightClient::new_for_test(wallet()).await;
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
}

#[cfg(test)]
mod proof_acquisition {
    //! The ProofAcquisition contract: the arbiter's evidence promotes or
    //! convicts the Standing Client's exit, the conviction runs the
    //! two-layer failover, and the expiry watchdog fires a new
    //! ProofAcquisition the moment the proof stops being epoch-fresh.

    use super::{adjudicate_standing_proof, standing_proof_watchdog};
    use crate::correspondent::pool::exit_pool::Reservation;
    use crate::mixnet::{Indicator, MixnetSlot, StandingClient};

    fn slot_with(born_probed: bool, exit: &str) -> std::sync::Arc<std::sync::Mutex<MixnetSlot>> {
        let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
            "127.0.0.1:9".parse().expect("the test address parses"),
            vec![crate::mixnet::ExitNodeId::from(exit)],
        );
        std::sync::Arc::new(std::sync::Mutex::new(MixnetSlot::Attached(
            StandingClient::new(
                proxy,
                Some(Reservation::dangling_for_test(exit)),
                born_probed,
            ),
        )))
    }

    /// HYPOTHESIS: an answered arbiter probe promotes stale proof to
    /// earned Ready and refreshes the exit's EpochProven observation.
    /// Falsified if an answer convicts, demotes, or writes nothing.
    #[tokio::test]
    async fn an_answer_promotes_and_refreshes_the_observation() {
        let slot = slot_with(false, "exit-answers");
        let pools = crate::correspondent::pool::Pools::new();
        let status = crate::mixnet::status_publisher();

        adjudicate_standing_proof(
            slot.clone(),
            pools.clone(),
            std::sync::Arc::clone(&status),
            zingo_netutils::sentinel::evidence_of(Some(64), 900),
        )
        .await;

        assert_eq!(slot.lock().unwrap().mode(), Indicator::Ready);
        assert!(
            pools.exits.lock().unwrap().epoch_proven(
                &crate::mixnet::ExitNodeId::from("exit-answers"),
                std::time::Instant::now()
            ),
            "the answer refreshes the exit's EpochProven observation"
        );
        assert_eq!(status.borrow().mode, Indicator::Ready);
    }

    /// HYPOTHESIS: a silent arbiter probe convicts the exit, and with no
    /// acquirer to rebirth from the failover exhausts into a latched Died,
    /// published to subscribers, with the conviction remembered. Falsified
    /// if silence leaves readiness standing or the conviction unwritten.
    #[tokio::test]
    async fn silence_convicts_and_a_rebirthless_failover_latches_died() {
        let slot = slot_with(true, "exit-silent");
        let pools = crate::correspondent::pool::Pools::new();
        let status = crate::mixnet::status_publisher();

        adjudicate_standing_proof(
            slot.clone(),
            pools.clone(),
            std::sync::Arc::clone(&status),
            zingo_netutils::sentinel::ExitEvidence::Silent,
        )
        .await;

        assert_eq!(slot.lock().unwrap().mode(), Indicator::Died);
        assert!(
            pools.exits.lock().unwrap().epoch_failed(
                &crate::mixnet::ExitNodeId::from("exit-silent"),
                std::time::Instant::now()
            ),
            "the conviction reaches the NodeHealthIndex"
        );
        assert_eq!(status.borrow().mode, Indicator::Died);
    }

    /// HYPOTHESIS: when the proof deadline lapses, the expiry watchdog
    /// fires a new ProofAcquisition at that earliest feasible moment: the
    /// arbiter probe runs against the tunnel (refused here, so silent),
    /// the exit is convicted, and the rebirthless failover latches Died —
    /// all without any operation touching the client. Falsified if expiry
    /// passes unnoticed.
    #[tokio::test(start_paused = true)]
    async fn expiry_fires_a_proof_acquisition_unprompted() {
        let slot = slot_with(true, "exit-expired");
        if let MixnetSlot::Attached(client) = &*slot.lock().unwrap() {
            client.set_proof_deadline(std::time::Instant::now());
        }
        let pools = crate::correspondent::pool::Pools::new();
        let status = crate::mixnet::status_publisher();
        let mut watcher = status.subscribe();

        let watchdog = tokio::spawn(standing_proof_watchdog(
            slot.clone(),
            pools.clone(),
            std::sync::Arc::clone(&status),
        ));
        watcher
            .wait_for(|published| published.mode == Indicator::Died)
            .await
            .expect("the watchdog publishes the exhausted failover");
        watchdog.abort();

        assert!(
            pools.exits.lock().unwrap().epoch_failed(
                &crate::mixnet::ExitNodeId::from("exit-expired"),
                std::time::Instant::now()
            ),
            "the unprompted ProofAcquisition convicted the lapsed exit"
        );
    }
}

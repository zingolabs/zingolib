//! TODO: Add Mod Description Here!

use std::convert::Infallible;
use std::future::Future;

use nonempty::NonEmpty;

use zcash_client_backend::data_api::wallet::TargetHeight;
use zcash_client_backend::proposal::{Proposal, ProposalError};
use zcash_client_backend::zip321::TransactionRequest;
use zcash_primitives::transaction::builder::DEFAULT_TX_EXPIRY_DELTA;
use zcash_primitives::transaction::{TxId, fees::zip317};
use zcash_protocol::consensus::BranchId;
use zcash_transparent::keys::NonHardenedChildIndex;

use pepper_sync::keys::transparent::{TransparentAddressId, TransparentScope};
use zingo_netutils::Indexer as _;
use zingo_netutils::lightwallet_protocol::{RawTransaction, TxFilter};
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::config::ChainType;
use crate::data::proposal::ZingoProposal;
use crate::lightclient::error::{LightClientError, SendError, TransmissionError};
use crate::lightclient::indexer_history::{
    AttemptKind, AttemptRoute, FailureKind, IndexerAttempt, IndexerHistoryHandle, now_unix_secs,
};
use crate::lightclient::transmit::{
    TransmitFailed, TransmitProgressHandle, TransmitProgressScope, TransmitTarget,
    resilient_transmit,
};

/// Records one finished send attempt against `host` into the cross-session
/// history: route, elapsed time, and the sanitized failure category when it
/// failed, never the raw failure prose, which can embed the txid.
fn record_send_attempt(
    history: &IndexerHistoryHandle,
    host: &crate::destination::Host,
    route: AttemptRoute,
    started: std::time::Instant,
    outcome: &Result<String, zingo_net_diag::NetOpFailure>,
    phase: Option<crate::destination::health::FailurePhase>,
) {
    history.record(&IndexerAttempt {
        unix_secs: now_unix_secs(),
        host: host.clone(),
        route,
        kind: AttemptKind::Send,
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        phase,
        outcome: match outcome {
            Ok(_) => Ok(()),
            Err(failure) => Err(FailureKind::classify(&failure.to_string())),
        },
    });
}

/// Why one transaction's transmission failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TransmitError {
    /// The clearnet arm has no configured indexer.
    #[error("clearnet transmission requires a configured indexer")]
    NoClearnetIndexer,
    /// A mixnet route arrived in a build without the `nym` feature.
    #[cfg(not(feature = "nym"))]
    #[error("a mixnet route requires the nym feature")]
    MixnetUnbuilt,
    /// The single-target transmit's taxonomy record.
    #[error(transparent)]
    Failure(#[from] zingo_net_diag::NetOpFailure),
    /// The Destination draw refused.
    #[cfg(feature = "nym")]
    #[error(transparent)]
    Draw(#[from] crate::destination::NoEligibleDestinations),
    /// Every arm of the escalation failed, reported whole.
    #[cfg(feature = "nym")]
    #[error("{0}")]
    Escalation(crate::mixnet::destination_rotation::EscalationError<zingo_net_diag::NetOpFailure>),
}

use crate::lightclient::{DEFAULT_REQUEST_TIMEOUT, LightClient};
use crate::wallet::error::WalletError;
use crate::wallet::output::OutputRef;

/// Attestation of one transmitted transaction: the route the bytes
/// traveled, the endpoint that accepted them, and the transmission's
/// round-trip time. It rides the success value — not a log — the same
/// doctrine as the nym-gated `MixnetPriceFetch` price attestation
/// (ADR 0011), so every consumer of a send holds per-transaction
/// evidence of the route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransmitReport {
    /// The transmitted transaction.
    pub txid: TxId,
    /// The route the transaction traveled and the endpoint that accepted
    /// it.
    pub route: TransmitRoute,
    /// Wall-clock time from dispatching the transmission to its delivery
    /// confirmation, retries and Destination escalation included.
    pub round_trip: std::time::Duration,
}

/// Resolves whether a transmission runs over the mixnet tunnel (`Some`
/// SOCKS5 address) or clearnet through the configured sync indexer
/// (`None`), from the session's connectivity and its Mixnet Mode route.
///
/// An Indexerless session transmits only over a ready mixnet (ruling
/// 2026-07-29): the Destination escalation needs no sync indexer, so
/// the ADR 0022 exclusion holds vacuously. A mixnet-less offline session
/// keeps the typed [`LightClientError::Offline`] refusal — an unattached
/// mixnet carries no online intent — while attached-but-not-ready states
/// (bootstrapping, died) surface their own typed error, so the caller
/// learns to wait or repair rather than to connect an indexer.
#[cfg(feature = "nym")]
fn resolve_transmit_route(
    has_indexer: bool,
    route: Result<crate::mixnet::MixnetRoute, crate::mixnet::MixnetNotReady>,
) -> Result<Option<zingo_netutils::conduit::ConduitDial>, LightClientError> {
    use crate::mixnet::{MixnetNotReady, MixnetRoute};
    match (has_indexer, route) {
        // The guard rides out to the caller, which holds it for the send.
        (_, Ok(MixnetRoute::Mixnet(conduit))) => Ok(Some(conduit.dial())),
        (true, Ok(MixnetRoute::Clearnet)) => Ok(None),
        (false, Ok(MixnetRoute::Clearnet)) => Err(LightClientError::Offline),
        (false, Err(MixnetNotReady::Unattached)) => Err(LightClientError::Offline),
        (_, Err(e)) => Err(LightClientError::MixnetNotReady(e)),
    }
}

/// The route one transmitted transaction traveled (ADR 0011).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransmitRoute {
    /// Clearnet submission through the session's configured sync indexer.
    Clearnet {
        /// The sync indexer's host.
        indexer: String,
    },
    /// Mixnet escalation over the Destinations (ADR 0022), reached
    /// through the local SOCKS5 tunnel endpoint.
    Mixnet {
        /// The host of the Destination whose delivery confirmation
        /// won the escalation.
        destination: String,
        /// The local SOCKS5 endpoint of the mixnet tunnel.
        via_socks5: String,
    },
}

/// ZIP 203: `nExpiryHeight` values at or above this threshold are
/// interpreted as a block time rather than a block height, so a
/// transaction's expiry height must stay strictly below it.
/// (`zcash_primitives` does not export the constant.)
pub(crate) const ZIP_203_EXPIRY_HEIGHT_THRESHOLD: u32 = 500_000_000;

/// Lifts a stored proposal's target height to the last height of the
/// consensus-branch epoch the wallet believes it is in, for offline signing.
///
/// The transaction built from a proposal expires at its target height plus
/// [`DEFAULT_TX_EXPIRY_DELTA`], so the lift gives it the longest expiry the
/// epoch permits: it stays transmittable until the next scheduled network
/// upgrade. That is the outer limit for any pre-signed Zcash transaction:
/// the signature commits to the epoch's consensus branch ID, so no expiry
/// height can carry it past the upgrade. When the params schedule no
/// upgrade above the stored target, the cap is instead the highest target
/// whose expiry ZIP 203 can encode as a height.
///
/// The steps and their anchors are copied untouched, and the stored target
/// is never lowered.
fn retarget_for_offline_signing<NoteRef: Clone>(
    proposal: &Proposal<zip317::FeeRule, NoteRef>,
    chain_type: &ChainType,
) -> Result<Proposal<zip317::FeeRule, NoteRef>, ProposalError> {
    let stored_target = proposal.min_target_height();
    let epoch = BranchId::for_height(chain_type, stored_target.into());
    let cap = match epoch.height_bounds(chain_type) {
        Some((_, Some(next_activation))) => u32::from(next_activation) - 1,
        _ => ZIP_203_EXPIRY_HEIGHT_THRESHOLD - 1 - DEFAULT_TX_EXPIRY_DELTA,
    };
    let lifted_target = cap.max(u32::from(stored_target));
    Proposal::multi_step(
        proposal.fee_rule().clone(),
        TargetHeight::from(lifted_target),
        proposal.confirmations_policy(),
        proposal.steps().clone(),
    )
}

/// The configured clearnet indexer as a [`TransmitTarget`]: it submits over the
/// ordinary gRPC channel and delivery-checks with `get_transaction`. The Nym
/// path supplies a SOCKS5-backed target to the same [`resilient_transmit`]
/// policy.
struct ClearnetTarget(zingo_netutils::GrpcIndexer);

impl TransmitTarget for ClearnetTarget {
    type Failure = zingo_netutils::Status;

    fn submit(
        &self,
        raw_tx: &[u8],
        height: u64,
    ) -> impl Future<Output = Result<String, zingo_netutils::Status>> + Send {
        let mut client = self.0.clone();
        let data = raw_tx.to_vec();
        async move {
            client
                .send_transaction(RawTransaction { data, height }, DEFAULT_REQUEST_TIMEOUT)
                .await
        }
    }

    fn knows_transaction(&self, txid: &TxId) -> impl Future<Output = bool> + Send {
        let mut client = self.0.clone();
        let hash = txid.as_ref().to_vec();
        async move {
            client
                .get_transaction(
                    TxFilter {
                        block: None,
                        index: 0,
                        hash,
                    },
                    DEFAULT_REQUEST_TIMEOUT,
                )
                .await
                .is_ok()
        }
    }
}

/// A [`zingo_netutils::Socks5Indexer`] is the mixnet [`TransmitTarget`]:
/// one Destination that submits and delivery-checks over its own tunnel,
/// running the same [`resilient_transmit`] policy as the clearnet path.
#[cfg(feature = "nym")]
impl TransmitTarget for zingo_netutils::Socks5Indexer {
    type Failure = zingo_netutils::Socks5TransmitError;

    async fn submit(
        &self,
        raw_tx: &[u8],
        height: u64,
    ) -> Result<String, zingo_netutils::Socks5TransmitError> {
        self.send_transaction(raw_tx, height).await
    }

    fn knows_transaction(&self, txid: &TxId) -> impl Future<Output = bool> + Send {
        let hash = txid.as_ref().to_vec();
        async move { self.transaction_known(&hash).await }
    }
}

/// The mixnet route one Transmission's pulls take: the session's standing
/// client, which every pull multiplexes over.
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "nym"), allow(dead_code))]
pub(crate) struct PullRoute {
    shared_socks5: std::net::SocketAddr,
}

/// Submit one transaction under the route the Mixnet Mode policy resolved:
/// clearnet through the configured indexer when `route` is `None`, or the
/// mixnet escalation over the Destinations when it is `Some`. Returns the
/// server-reported txid or the last failure message.
/// The ambient state a transmission narrates through, records against, and paces itself by.
struct TransmitContext<'a> {
    progress: &'a TransmitProgressHandle,
    history: &'a IndexerHistoryHandle,
    retry_interval: std::time::Duration,
}

async fn transmit_one_transaction(
    route: Option<PullRoute>,
    indexer: Option<&zingo_netutils::GrpcIndexer>,
    tx_bytes: &[u8],
    height: u64,
    txid: &TxId,
    context: &TransmitContext<'_>,
) -> Result<(String, TransmitRoute), TransmitError> {
    match route {
        None => {
            // The route resolver refuses an Indexerless clearnet route
            // before any transaction is built, so this arm always holds one.
            let Some(indexer) = indexer else {
                return Err(TransmitError::NoClearnetIndexer);
            };
            let host = crate::destination::Host::of_uri(indexer.uri());
            let started = std::time::Instant::now();
            let outcome = resilient_transmit(
                &ClearnetTarget(indexer.clone()),
                tx_bytes,
                height,
                txid,
                move |_| tokio::time::sleep(context.retry_interval),
                |event| context.progress.set(format!("indexer {host}: {event}")),
            )
            .await
            .map_err(|TransmitFailed(status)| {
                zingo_net_diag::NetOpFailure::from_error(
                    zingo_net_diag::NetOpStage::RemoteHttp,
                    &host,
                    &status,
                )
            });
            record_send_attempt(
                context.history,
                &host,
                AttemptRoute::Clearnet,
                started,
                &outcome,
                // A clearnet attempt rides no tunnel, so every failure it
                // sees is the indexer's own.
                outcome
                    .is_err()
                    .then_some(crate::destination::health::FailurePhase::Destination),
            );
            outcome
                .map(|server_txid| {
                    (
                        server_txid,
                        TransmitRoute::Clearnet {
                            indexer: host.to_string(),
                        },
                    )
                })
                .map_err(TransmitError::from)
        }
        #[cfg(feature = "nym")]
        Some(route) => {
            mixnet_escalating_transmit(
                route,
                indexer.map(|indexer| indexer.uri()),
                tx_bytes,
                height,
                txid,
                context,
            )
            .await
        }
        #[cfg(not(feature = "nym"))]
        Some(_) => Err(TransmitError::MixnetUnbuilt),
    }
}

/// Transmit one transaction over the mixnet as the escalating, serially gated
/// Destination Rotation (ADR 0011): each arm runs the shared
/// [`resilient_transmit`] policy against one Destination through the SOCKS5
/// proxy, and the escalation widens round by round until a Destination
/// confirms delivery or the cap is reached.
///
/// The draw comes from [`crate::destination::eligible_destinations`],
/// never the raw curated list: a Destination is never the sync indexer's
/// operator (ADR 0022), because that party already holds the wallet's address
/// set and must not receive the transmission too. An emptied pool refuses
/// rather than falling back.
#[cfg(feature = "nym")]
async fn mixnet_escalating_transmit(
    route: PullRoute,
    sync_indexer: Option<&http::Uri>,
    tx_bytes: &[u8],
    height: u64,
    txid: &TxId,
    context: &TransmitContext<'_>,
) -> Result<(String, TransmitRoute), TransmitError> {
    use crate::destination::eligible_destinations;
    use crate::mixnet::destination_rotation::{MAX_TRANSMISSION_DESTINATIONS, escalating_transmit};

    let indexers = eligible_destinations(
        sync_indexer,
        &context.history.health().lock().expect("health mutex"),
    )?;
    let run_pull = |indexer: http::Uri| {
        let socks5_addr = route.shared_socks5;
        let tx_bytes = tx_bytes.to_vec();
        let txid = *txid;
        let host = crate::destination::Host::of_uri(&indexer);
        async move {
            // Every pull multiplexes over the session's standing client,
            // whose exit was proven at its birth; the standing
            // client is one egress for all wallet-correlated streams.
            let target =
                zingo_netutils::Socks5Indexer::new(socks5_addr, indexer, DEFAULT_REQUEST_TIMEOUT);
            let started = std::time::Instant::now();
            // The pull's failure becomes the taxonomy record — stage by typed
            // match, cause chain captured layer by layer, target the
            // Destination host — which the escalation collects whole per
            // Destination.
            let outcome = resilient_transmit(
                &target,
                &tx_bytes,
                height,
                &txid,
                move |_| tokio::time::sleep(context.retry_interval),
                |event| context.progress.set(format!("destination {host}: {event}")),
            )
            .await
            .map_err(|TransmitFailed(error)| crate::mixnet::socks5_transmit_failure(&error, &host));
            record_send_attempt(
                context.history,
                &host,
                AttemptRoute::Mixnet,
                started,
                &outcome,
                outcome
                    .as_ref()
                    .err()
                    .map(|failure| crate::mixnet::charge_phase(&failure.stage)),
            );
            outcome.map(|server_txid| {
                (
                    server_txid,
                    TransmitRoute::Mixnet {
                        destination: host.to_string(),
                        via_socks5: socks5_addr.to_string(),
                    },
                )
            })
        }
    };

    escalating_transmit(
        &indexers,
        &mut rand::rngs::OsRng,
        MAX_TRANSMISSION_DESTINATIONS,
        run_pull,
        |line| context.progress.set(format!("mixnet escalation: {line}")),
    )
    .await
    .map_err(TransmitError::Escalation)
}

/// The chain-mock twin of [`mixnet_escalating_transmit`], paired with the
/// test-attached slot state behind
/// [`LightClient::switch_on_mixnet_for_tests`]: the Destination draw, the
/// escalation rounds, and the cap run for real over the curated Destination
/// pool, while each arm's bytes travel the mock indexer's channel
/// instead of a SOCKS5 tunnel. The tunnel's byte transport is pinned by
/// zingo-netutils' own tests, so no packet leaves the process here.
#[cfg(all(feature = "nym", any(test, feature = "testutils")))]
async fn mock_escalating_transmit(
    indexer: &zingo_netutils::GrpcIndexer,
    tx_bytes: &[u8],
    height: u64,
    txid: &TxId,
    context: &TransmitContext<'_>,
) -> Result<(String, String), TransmitError> {
    use crate::destination::eligible_destinations;
    use crate::mixnet::destination_rotation::{MAX_TRANSMISSION_DESTINATIONS, escalating_transmit};

    let destinations = eligible_destinations(
        Some(indexer.uri()),
        &context.history.health().lock().expect("health mutex"),
    )?;
    let run_arm = |destination: http::Uri| {
        let target = ClearnetTarget(indexer.clone());
        let tx_bytes = tx_bytes.to_vec();
        let txid = *txid;
        let host = crate::destination::Host::of_uri(&destination);
        async move {
            let started = std::time::Instant::now();
            let outcome = resilient_transmit(
                &target,
                &tx_bytes,
                height,
                &txid,
                move |_| tokio::time::sleep(context.retry_interval),
                |event| context.progress.set(format!("destination {host}: {event}")),
            )
            .await
            .map_err(|TransmitFailed(status)| {
                zingo_net_diag::NetOpFailure::from_error(
                    zingo_net_diag::NetOpStage::RemoteHttp,
                    &host,
                    &status,
                )
            });
            record_send_attempt(
                context.history,
                &host,
                AttemptRoute::Mixnet,
                started,
                &outcome,
                None,
            );
            outcome.map(|server_txid| (server_txid, host.to_string()))
        }
    };

    escalating_transmit(
        &destinations,
        &mut rand::rngs::OsRng,
        MAX_TRANSMISSION_DESTINATIONS,
        run_arm,
        |line| context.progress.set(format!("mixnet escalation: {line}")),
    )
    .await
    .map_err(TransmitError::Escalation)
}

impl LightClient {
    async fn send(
        &mut self,
        proposal: Proposal<zip317::FeeRule, OutputRef>,
        sending_account: zip32::AccountId,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        self.preflight_transmit()?;
        let indexerless = self.indexer.is_none();
        let mut wallet = self.wallet().write().await;
        // An Indexerless calculation cannot trust the wallet's stale chain
        // view for an ordinary tip-plus-delta expiry, so it takes the
        // epoch-limit retarget the offline-signing flow uses (issue #2455).
        let proposal = if indexerless {
            let chain_type = wallet.chain_type();
            retarget_for_offline_signing(&proposal, &chain_type)
                .map_err(SendError::RetargetError)?
        } else {
            proposal
        };
        let highest_refund_address_index = wallet.highest_refund_address_index();
        let calculated_txids = wallet
            .calculate_transactions(proposal, sending_account, None)
            .await
            .map_err(|e| {
                wallet.truncate_refund_addresses(highest_refund_address_index);

                SendError::CalculateSendError(e)
            })?;
        drop(wallet);

        let transmission_result = self.transmit_transactions(calculated_txids).await;
        if transmission_result.is_err() {
            let mut wallet = self.wallet().write().await;
            let new_refund_address_index = highest_refund_address_index
                .map_or(Some(NonHardenedChildIndex::ZERO), |i| i.next());
            let new_refund_address = new_refund_address_index.and_then(|i| {
                wallet
                    .transparent_addresses()
                    .get(&TransparentAddressId::new(
                        sending_account,
                        TransparentScope::Refund,
                        i,
                    ))
                    .cloned()
            });
            let truncate = new_refund_address.is_some_and(|addr| {
                let deshielding_tx = wallet.wallet_transactions.values().find(|tx| {
                    tx.transparent_coins()
                        .iter()
                        .any(|coin| coin.address() == addr)
                });
                deshielding_tx.is_some_and(|tx| tx.status().is_failed())
            });
            if truncate {
                wallet.truncate_refund_addresses(highest_refund_address_index);
            }
        }

        transmission_result
    }

    async fn shield(
        &mut self,
        proposal: Proposal<zip317::FeeRule, Infallible>,
        shielding_account: zip32::AccountId,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        let calculated_txids = self
            .wallet()
            .write()
            .await
            // A Shield never carries an OP_RETURN payload.
            .calculate_transactions(proposal, shielding_account, None)
            .await
            .map_err(SendError::CalculateShieldError)?;

        self.transmit_transactions(calculated_txids).await
    }

    /// Creates and transmits transactions from a stored proposal.
    ///
    /// If sync was running prior to creating a send proposal, sync will have
    /// been paused. If `resume_sync` is `true`, the engine is restored to
    /// the mode it held before the proposal was created, so a pause the
    /// caller established before proposing is preserved, not overridden. If
    /// `false`, the engine stays paused for the caller to resume.
    pub async fn send_stored_proposal(
        &mut self,
        resume_sync: bool,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        self.require_indexer()?;
        let opt_proposal = self.wallet().write().await.take_proposal();
        if let Some(proposal) = opt_proposal {
            let reports = match proposal {
                ZingoProposal::Send {
                    proposal,
                    sending_account,
                } => self.send(proposal, sending_account).await,
                ZingoProposal::Shield {
                    proposal,
                    shielding_account,
                } => self.shield(proposal, shielding_account).await,
            };

            self.release_proposal_pause(resume_sync);

            reports.map(|reports| reports.map(|report| report.txid))
        } else {
            Err(SendError::NoStoredProposal.into())
        }
    }

    /// Calculates (signs) transactions from the stored proposal without an
    /// Indexer, the offline-signing half of the Indexerless capability set
    /// (ADR 0006). The stored proposal is consumed, exactly as
    /// [`Self::send_stored_proposal`] consumes it, and the signed
    /// transactions land in the wallet with `Calculated` status. Transmit
    /// them with [`Self::transmit_calculated`] once an Indexer is
    /// configured.
    ///
    /// When the client is Indexerless, the stored proposal is first
    /// retargeted: its target height is lifted to the last height of the
    /// consensus-branch epoch the wallet believes it is in, so the built
    /// transaction carries the longest expiry that epoch permits. It stays
    /// transmittable until the next scheduled network upgrade, the outer
    /// limit for any pre-signed Zcash transaction, whose signature commits
    /// to the epoch's consensus branch ID, and a stale offline chain view
    /// cannot expire it before an Indexer is available (issue #2455). An
    /// Indexer-connected calculation keeps the proposal's ordinary expiry,
    /// [`DEFAULT_TX_EXPIRY_DELTA`] blocks past the target: connected
    /// callers are expected to transmit promptly.
    pub async fn calculate_stored_proposal(&mut self) -> Result<NonEmpty<TxId>, LightClientError> {
        let indexerless = self.indexer.is_none();
        let mut wallet = self.wallet().write().await;
        let opt_proposal = wallet.take_proposal();
        let Some(proposal) = opt_proposal else {
            return Err(SendError::NoStoredProposal.into());
        };
        let chain_type = wallet.chain_type();
        let result = match proposal {
            ZingoProposal::Send {
                proposal,
                sending_account,
            } => {
                let retargeted = if indexerless {
                    retarget_for_offline_signing(&proposal, &chain_type)
                        .map_err(SendError::RetargetError)
                } else {
                    Ok(proposal)
                };
                match retargeted {
                    Ok(proposal) => wallet
                        .calculate_transactions(proposal, sending_account, None)
                        .await
                        .map_err(SendError::CalculateSendError),
                    Err(e) => Err(e),
                }
            }
            ZingoProposal::Shield {
                proposal,
                shielding_account,
            } => {
                let retargeted = if indexerless {
                    retarget_for_offline_signing(&proposal, &chain_type)
                        .map_err(SendError::RetargetError)
                } else {
                    Ok(proposal)
                };
                match retargeted {
                    Ok(proposal) => wallet
                        // A Shield never carries an OP_RETURN payload.
            .calculate_transactions(proposal, shielding_account, None)
                        .await
                        .map_err(SendError::CalculateShieldError),
                    Err(e) => Err(e),
                }
            }
        };
        drop(wallet);
        // The proposal is consumed on every path above, so its pause
        // has nothing left to guard; the engine returns to its prior mode.
        self.release_proposal_pause(true);
        result.map_err(LightClientError::from)
    }

    /// Pre-flights the transmission route without transmitting, so a route
    /// that would refuse is caught before any transaction is built and no
    /// freshly Calculated transaction is stranded. The same resolution
    /// [`Self::transmit_transactions`] performs for real: clearnet demands
    /// the configured indexer, and an Indexerless session passes only with
    /// a ready mixnet (ruling 2026-07-29).
    fn preflight_transmit(&self) -> Result<(), LightClientError> {
        #[cfg(feature = "nym")]
        {
            resolve_transmit_route(self.indexer.is_some(), self.mixnet_route()).map(|_| ())
        }
        #[cfg(not(feature = "nym"))]
        {
            self.require_indexer().map(|_| ())
        }
    }

    /// Transmits previously calculated transactions to the Indexer, in the
    /// given order, the transmission half of the offline-signing flow.
    /// Requires an Indexer. An Indexerless attempt fails with
    /// [`LightClientError::Offline`] and the Calculated transactions remain
    /// in the wallet, ready to transmit once connected.
    pub async fn transmit_calculated(
        &mut self,
        calculated_txids: NonEmpty<TxId>,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        self.transmit_transactions(calculated_txids)
            .await
            .map(|reports| reports.map(|report| report.txid))
    }

    /// Proposes and transmits transactions from a transaction request skipping proposal confirmation.
    ///
    /// If sync is running, it is paused before creating the send proposal.
    /// If `resume_sync` is `true`, the engine is restored to its prior mode
    /// after the send, on every exit path. If `false`, it stays paused for
    /// the caller to resume.
    pub async fn quick_send(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
        resume_sync: bool,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        self.quick_send_reported(request, account_id, resume_sync)
            .await
            .map(|reports| reports.map(|report| report.txid))
    }

    /// [`Self::quick_send`] with the transmission attested: each
    /// transaction's [`TransmitReport`] carries the route it traveled, the
    /// endpoint that accepted it, and the transmission's round-trip time.
    pub async fn quick_send_reported(
        &mut self,
        request: TransactionRequest,
        account_id: zip32::AccountId,
        resume_sync: bool,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        // Proposing is an Indexerless capability; only the calculate/transmit
        // stage below demands a connection.
        let guard = self.pause_sync_scoped().ok();
        let proposal_result = self
            .wallet()
            .write()
            .await
            .create_send_proposal(request, account_id)
            .map_err(SendError::ProposeSendError);
        let reports = match proposal_result {
            Ok(proposal) => self.send(proposal, account_id).await,
            Err(e) => Err(e.into()),
        };
        if let Some(guard) = guard
            && !resume_sync
        {
            guard.disarm();
        }

        reports
    }

    /// Shields all transparent funds skipping proposal confirmation. The
    /// sync engine is paused before the proposal's wallet reads and
    /// restored to its prior mode when the call returns. The shield path
    /// previously proposed, built, and stored transactions under a running
    /// engine.
    pub async fn quick_shield(
        &mut self,
        account_id: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        // Proposing is an Indexerless capability; only the calculate/transmit
        // stage below demands a connection.
        let _guard = self.pause_sync_scoped().ok();
        let proposal = self
            .wallet()
            .write()
            .await
            .create_shield_proposal(account_id)
            .map_err(SendError::ProposeShieldError)?;

        self.shield(proposal, account_id)
            .await
            .map(|reports| reports.map(|report| report.txid))
    }

    /// Tranmits calculated transactions stored in the wallet matching txids of `calculated_txids` in the given order.
    /// Returns list of txids for successfully transmitted transactions.
    pub(crate) async fn transmit_transactions(
        &mut self,
        calculated_txids: NonEmpty<TxId>,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        let indexer = self.indexer.clone();

        // Resolve the Mixnet Mode route once for the whole send (ADR 0011).
        // `Clearnet` submits through the configured indexer; `Mixnet(conduit)`
        // routes the escalation through the conduit's SOCKS5 proxy — with or without a
        // sync indexer (ruling 2026-07-29); `Bootstrapping` fails closed
        // here, before any submission, rather than leaking to clearnet.
        // Without the `nym` feature there is no mixnet, so the route is
        // clearnet and demands the indexer.
        // The guard is bound for the whole send, so the conduit counts this
        // transmission as outstanding until the escalation finishes.
        #[cfg(feature = "nym")]
        let transmit_dial = resolve_transmit_route(indexer.is_some(), self.mixnet_route())?;
        #[cfg(feature = "nym")]
        let socks5_proxy: Option<std::net::SocketAddr> =
            transmit_dial.as_ref().map(|dial| dial.socks5());
        #[cfg(not(feature = "nym"))]
        let socks5_proxy: Option<std::net::SocketAddr> = None;
        // Every pull rides the session's standing client on both platforms:
        // one proven egress for all wallet-correlated streams.
        let pull_route = socks5_proxy.map(|shared_socks5| PullRoute { shared_socks5 });
        if socks5_proxy.is_none() && indexer.is_none() {
            return Err(LightClientError::Offline);
        }

        // A test-attached slot pairs its Ready route with arms that submit
        // over the mock indexer's channel; a live Ready session keeps the
        // SOCKS5 escalation. Production builds carry no test slot state, so
        // this distinction does not exist there.
        #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
        let mock_arms = matches!(
            *self.mixnet_slot.lock().expect("mixnet slot mutex"),
            crate::mixnet::MixnetSlot::AttachedForTests { .. }
        );

        // Narrate the transmission into the side channel; the scope clears it
        // on every exit so no stale line outlives this call.
        let progress = self.transmit_progress.clone();
        let _progress_scope = TransmitProgressScope(progress.clone());
        let history = self.indexer_history.clone();
        let total = calculated_txids.len();
        let mut reports: Vec<TransmitReport> = Vec::with_capacity(total);

        let mut wallet = self.wallet().write().await;
        for (index, txid) in calculated_txids.iter().enumerate() {
            progress.set(format!("transaction {} of {total}: preparing", index + 1));
            let calculated_transaction = wallet
                .wallet_transactions
                .get(txid)
                .ok_or(WalletError::TransactionNotFound(*txid))?;
            let height = calculated_transaction.status().get_height();

            if !matches!(
                calculated_transaction.status(),
                ConfirmationStatus::Calculated(_)
            ) {
                return Err(SendError::TransmissionError(
                    TransmissionError::IncorrectTransactionStatus(*txid),
                )
                .into());
            }

            let mut transaction_bytes = vec![];
            calculated_transaction
                .transaction()
                .write(&mut transaction_bytes)
                .map_err(|e| {
                    pepper_sync::set_transactions_failed(
                        &mut wallet.wallet_transactions,
                        vec![*txid],
                    );
                    wallet.save_required = true;
                    WalletError::TransactionWrite(e)
                })?;

            // The retry / duplicate-in-mempool / queued-probe policy is defined
            // once in `transmit::resilient_transmit`; the clearnet path runs it
            // directly and the mixnet path runs it per escalation arm.
            // Wallet-state effects stay here, around the pure transmission.
            let dispatched = std::time::Instant::now();
            let transmit_context = TransmitContext {
                progress: &progress,
                history: &history,
                retry_interval: self.transmit_retry_interval,
            };
            #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
            let transmit_outcome = if mock_arms {
                mock_escalating_transmit(
                    indexer
                        .as_ref()
                        .expect("the test-attached slot always carries a mock indexer"),
                    &transaction_bytes,
                    height.into(),
                    txid,
                    &transmit_context,
                )
                .await
                .map(|(server_txid, destination)| {
                    (
                        server_txid,
                        TransmitRoute::Mixnet {
                            destination,
                            via_socks5: socks5_proxy
                                .map(|addr| addr.to_string())
                                .unwrap_or_default(),
                        },
                    )
                })
            } else {
                transmit_one_transaction(
                    pull_route,
                    indexer.as_ref(),
                    &transaction_bytes,
                    height.into(),
                    txid,
                    &transmit_context,
                )
                .await
            };
            #[cfg(not(all(feature = "nym", any(test, feature = "testutils"))))]
            let transmit_outcome = transmit_one_transaction(
                pull_route,
                indexer.as_ref(),
                &transaction_bytes,
                height.into(),
                txid,
                &transmit_context,
            )
            .await;
            let (txid_from_server, route) = match transmit_outcome {
                Ok(server_txid_and_route) => {
                    // A delivered mixnet transmission is a completed round
                    // trip through the Standing Client, promoting stale
                    // proof to earned.
                    #[cfg(feature = "nym")]
                    if matches!(server_txid_and_route.1, TransmitRoute::Mixnet { .. }) {
                        self.note_standing_round_trip();
                    }
                    server_txid_and_route
                }
                Err(failure) => {
                    // A failed mixnet transmission raises the suspicion that
                    // the standing exit is dead; the arbiter probe
                    // adjudicates rather than convicting on one failure.
                    #[cfg(feature = "nym")]
                    if pull_route.is_some() {
                        self.note_standing_exit_suspicion();
                    }
                    pepper_sync::set_transactions_failed(
                        &mut wallet.wallet_transactions,
                        vec![*txid],
                    );
                    wallet.save_required = true;
                    // The typed failure is rendered only here, at the
                    // report's existing prose field.
                    return Err(SendError::TransmissionError(
                        TransmissionError::TransmissionFailed(failure.to_string()),
                    )
                    .into());
                }
            };

            wallet
                .wallet_transactions
                .get_mut(txid)
                .ok_or(WalletError::TransactionNotFound(*txid))?
                .update_status(
                    ConfirmationStatus::Transmitted(height),
                    crate::utils::now(),
                    false,
                );
            wallet.save_required = true;

            let txid_from_server =
                crate::utils::conversion::txid_from_hex_encoded_str(txid_from_server.as_str())
                    .map_err(WalletError::ConversionFailed)?;
            if txid_from_server != *txid {
                return Err(SendError::TransmissionError(
                    TransmissionError::IncorrectTxidFromServer(*txid, txid_from_server),
                )
                .into());
            }

            // Published after the transaction is confirmed transmitted and its
            // server txid verified. Each is a no-op unless its owner armed the
            // side channel (an immediate migration or a note-splitting round),
            // and the two are mutually exclusive in practice.
            self.immediate_migration_progress.set_sent(index as u32 + 1);
            self.split_progress.set_sent(index as u32 + 1);

            reports.push(TransmitReport {
                txid: *txid,
                route,
                round_trip: dispatched.elapsed(),
            });
        }

        Ok(NonEmpty::from_vec(reports).expect("one report per calculated transaction"))
    }
}

/// Gap-4 cells of the protection audit's remediation plan
/// (docs/testing/test-protection-audit-dev-to-ironwood.md § Gap
/// remediation plan): the built transaction's expiry and consensus
/// branch id must derive from the wallet's synced height + 1.
/// `LightWallet::calculate_transactions` is the build-without-transmit
/// seam (it proves and stores the transaction without transmitting),
/// so these cells run offline over a synthetic wallet.
#[cfg(test)]
mod transmit_error_seam {
    use super::*;

    /// The chain height a seam test hands the transmitter; nothing on the
    /// refusal path reads it.
    const ARBITRARY_HEIGHT: u64 = 0;

    /// HYPOTHESIS: a send attempt's failure reaches the history as the typed
    /// taxonomy record, classified whole rather than from hand-rendered
    /// prose. Falsified if the recorded category drifts.
    #[test]
    fn send_attempt_failure_is_classified_from_the_record() {
        let history = IndexerHistoryHandle::default();
        let failure = zingo_net_diag::NetOpFailure::message(
            zingo_net_diag::NetOpStage::RemoteConnect,
            "indexer.example",
            "connection refused",
        );
        record_send_attempt(
            &history,
            &crate::destination::Host::of_host_str("indexer.example"),
            AttemptRoute::Clearnet,
            std::time::Instant::now(),
            &Err(failure),
            None,
        );
        let recorded = history.load();
        assert_eq!(recorded.len(), 1, "one attempt is recorded");
        assert_eq!(
            recorded[0].outcome,
            Err(crate::lightclient::indexer_history::FailureKind::Unreachable),
            "the category comes from the typed record"
        );
    }

    /// HYPOTHESIS: the clearnet arm without a configured indexer refuses as
    /// the typed variant before any network touch. Falsified if the refusal
    /// is any other variant.
    #[tokio::test]
    async fn missing_clearnet_indexer_refuses_typed() {
        let history = IndexerHistoryHandle::default();
        let progress = TransmitProgressHandle::default();
        let refusal = transmit_one_transaction(
            None,
            None,
            &[],
            ARBITRARY_HEIGHT,
            &TxId::from_bytes([0u8; 32]),
            &TransmitContext {
                progress: &progress,
                history: &history,
                retry_interval: zingo_netutils::time::TRANSMIT_RETRY_INTERVAL,
            },
        )
        .await
        .expect_err("no indexer must refuse");
        assert!(matches!(refusal, TransmitError::NoClearnetIndexer));
    }
}

#[cfg(test)]
mod built_transaction_shape {
    use zcash_protocol::consensus::{BlockHeight, BranchId};
    use zingo_common_components::protocol::ActivationHeights;
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::lightclient::LightClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::utils::conversion::address_from_str;
    use crate::wallet::LightWallet;
    use crate::wallet::keys::unified::ReceiverSelection;

    /// An orchard address of a different wallet, so the send is external.
    fn external_orchard_address() -> zcash_address::ZcashAddress {
        let mut external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let (_, unified_address) = external_wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        address_from_str(&unified_address.encode(&external_wallet.chain_type())).unwrap()
    }

    /// Builds (without transmitting) one send-all from the given wallet
    /// and returns the stored transaction's (target, expiry, branch id).
    async fn build_one_send(wallet: LightWallet) -> (u32, u32, BranchId) {
        let mut client = LightClient::new_for_test(wallet).await;
        let proposal = client
            .propose_send_all(
                external_orchard_address(),
                false,
                None,
                zip32::AccountId::ZERO,
            )
            .await
            .unwrap();
        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO, None)
            .await
            .unwrap();
        assert_eq!(txids.len(), 1);
        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let record = wallet.wallet_transactions.get(&txids[0]).unwrap();
        let ConfirmationStatus::Calculated(target) = record.status() else {
            panic!("a built, untransmitted transaction is stored as Calculated");
        };
        let transaction = record.transaction();
        (
            u32::from(target),
            u32::from(transaction.expiry_height()),
            transaction.consensus_branch_id(),
        )
    }

    /// The plain cell: synced to the default tip, the build targets
    /// tip + 1, expires the standard forty blocks later, and commits to
    /// the branch id in force at the target.
    #[tokio::test]
    async fn expiry_and_branch_id_derive_from_synced_height() {
        let tip = 20;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .tip(tip)
            .build();
        let chain = wallet.chain_type();

        let (target, expiry, branch_id) = build_one_send(wallet).await;

        assert_eq!(target, tip + 1);
        assert_eq!(expiry, target + 40, "standard tx expiry delta");
        assert_eq!(
            branch_id,
            BranchId::for_height(&chain, BlockHeight::from_u32(tip + 1))
        );
    }

    /// Offline twin of libtonode `fast::mine_to_transparent_and_shield`,
    /// which stays live as the pipeline control (its coinbase provenance
    /// and the documented shield-eligibility race are inexpressible
    /// offline): four transparent coins shield in one step, and the
    /// built transaction nets exactly their sum minus the 30_000
    /// four-input shield fee into the Ironwood pool (a V6 shield's
    /// change lands in the ironwood bundle, ADR 0009). The live assert
    /// is the post-confirmation balance. The offline equivalent is the
    /// ironwood bundle's value balance on the built transaction.
    #[tokio::test]
    async fn four_coin_shield_builds_and_nets_input_minus_fee() {
        let coin_value = 1_000_000u64;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .transparent_coin(coin_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let proposal = client.propose_shield(zip32::AccountId::ZERO).await.unwrap();
        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO, None)
            .await
            .unwrap();
        assert_eq!(txids.len(), 1);

        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let transaction = wallet
            .wallet_transactions
            .get(&txids[0])
            .unwrap()
            .transaction();
        let transparent = transaction
            .transparent_bundle()
            .expect("a shield spends transparent coins");
        assert_eq!(transparent.vin.len(), 4, "all four coins consumed");
        assert!(
            transparent.vout.is_empty(),
            "a shield pays no transparent outputs"
        );
        let ironwood = transaction
            .ironwood_bundle()
            .expect("a V6 shield produces ironwood change");
        // Negative value balance is value flowing INTO the ironwood pool:
        // the four coins minus the 30_000 zip317 fee (four transparent
        // inputs plus the ironwood action pair).
        assert_eq!(
            i64::from(ironwood.value_balance()),
            -i64::try_from(4 * coin_value - 30_000).unwrap()
        );
    }

    /// Gap-1b cell of the remediation plan, mirroring the live
    /// multi_input_sapling_send_with_orchard_change_no_panic offline: a
    /// payment that no single sapling note covers builds (proves) a
    /// two-input sapling spend. Under V6 the change stays in Sapling (the
    /// upstream change selector avoids pool-crossing when no orchard
    /// flow exists, ADR 0009) while the payment to the orchard receiver
    /// lands in the ironwood bundle. The sapling proving parameters are
    /// embedded in the crate, so the plan's parameters precondition is
    /// satisfied in the unit environment.
    #[tokio::test]
    async fn two_input_sapling_spend_with_sapling_change_builds_offline() {
        use zcash_client_backend::zip321::{Payment, TransactionRequest};
        use zcash_protocol::value::Zatoshis;

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .sapling_note(20_000)
            .sapling_note(30_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        // 25_000 plus the 20_000 ZIP-317 fee (two sapling spends covering
        // the change, plus the ironwood payment pair) exceeds either note
        // alone, so both are gathered and 5_000 returns as sapling change.
        let request = TransactionRequest::new(vec![Payment::without_memo(
            external_orchard_address(),
            Zatoshis::const_from_u64(25_000),
        )])
        .unwrap();
        let proposal = client
            .propose_send(request, zip32::AccountId::ZERO)
            .await
            .unwrap();
        let step = proposal.steps().first();
        let change = step.balance().proposed_change();
        assert_eq!(change.len(), 1);
        assert_eq!(u64::from(change[0].value()), 5_000);
        assert_eq!(change[0].output_pool(), zcash_protocol::PoolType::IRONWOOD,);

        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO, None)
            .await
            .unwrap();
        assert_eq!(txids.len(), 1);

        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let transaction = wallet
            .wallet_transactions
            .get(&txids[0])
            .unwrap()
            .transaction();
        let sapling_bundle = transaction
            .sapling_bundle()
            .expect("spending sapling notes produces a sapling bundle");
        assert_eq!(
            sapling_bundle.shielded_spends().len(),
            2,
            "both fabricated sapling notes are spent"
        );
        // The sapling bundle carries the spends and the change output;
        // the payment to the orchard receiver lands in the ironwood
        // bundle, and no legacy orchard bundle exists.
        assert!(
            transaction.orchard_bundle().is_none(),
            "no orchard flow, no orchard bundle"
        );
        let ironwood_bundle = transaction
            .ironwood_bundle()
            .expect("the payment to the orchard receiver produces an ironwood bundle");
        assert_eq!(
            ironwood_bundle.actions().len(),
            2,
            "payment plus dummy padding, the bundle minimum"
        );
    }

    /// Privacy invariant: spending a legacy (V5) Orchard note on a post-NU6.3
    /// chain must keep the **change** in the Orchard pool as a real Orchard
    /// output, not migrate it to Ironwood. ZIP 318 disables ordinary *payments*
    /// into the Orchard pool "while still permitting change"; the turnstile
    /// (ZIP 2006) only blocks value *entering* Orchard, and change funded by an
    /// Orchard input nets the pool *down*, so the Orchard change output is
    /// consensus-valid. Upstream's `SingleOutputChangeStrategy` already keeps
    /// change in the spent note's pool to avoid a pool-crossing (the
    /// `ShieldedPool::Ironwood` passed by the proposer is only the *fallback*
    /// for transactions with no shielded inputs, e.g. shields, ADR 0009);
    /// deliberate Orchard->Ironwood movement is the migration engine's job.
    ///
    /// The proof is at the built-transaction level, not the proposal label:
    /// the Orchard bundle carries the spend *and* the change output (its value
    /// balance nets out only the payment plus fee, so the change stays in the
    /// pool), while only the payment to the recipient's Orchard receiver routes
    /// through the Ironwood bundle (turnstiled per ZIP 318). Had the change
    /// crossed to Ironwood, the Orchard value balance would equal the whole
    /// spent note.
    #[tokio::test]
    async fn orchard_note_send_keeps_change_in_orchard_pool_post_nu6_3() {
        use zcash_client_backend::zip321::{Payment, TransactionRequest};
        use zcash_protocol::{PoolType, value::Zatoshis};

        let note_value = 200_000u64;
        let sent_value = 50_000u64;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(note_value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let request = TransactionRequest::new(vec![Payment::without_memo(
            external_orchard_address(),
            Zatoshis::const_from_u64(sent_value),
        )])
        .unwrap();
        let proposal = client
            .propose_send(request, zip32::AccountId::ZERO)
            .await
            .unwrap();
        let step = proposal.steps().first();
        let change = step.balance().proposed_change();
        assert_eq!(change.len(), 1);
        let change_value = u64::from(change[0].value());
        assert!(change_value > 0, "the send must leave real change to place");
        assert_eq!(
            change[0].output_pool(),
            PoolType::ORCHARD,
            "the proposer must select the Orchard pool for change when spending an Orchard note"
        );

        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO, None)
            .await
            .unwrap();
        assert_eq!(txids.len(), 1);
        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let transaction = wallet
            .wallet_transactions
            .get(&txids[0])
            .unwrap()
            .transaction();

        // Orchard bundle: spends the note and holds the change output. Its
        // value balance is (note - change) = payment + fee, which is strictly
        // less than the whole note — the discriminator proving the change is a
        // real Orchard output that stayed in the pool rather than crossing to
        // Ironwood (which would leave the value balance at the full note value).
        let orchard = transaction
            .orchard_bundle()
            .expect("spending a legacy Orchard note keeps a real Orchard bundle");
        assert_eq!(
            i64::from(*orchard.value_balance()),
            i64::try_from(note_value - change_value).unwrap(),
            "Orchard value balance must net out only payment + fee, leaving the change in Orchard"
        );

        // Ironwood bundle: carries only the payment to the recipient's Orchard
        // receiver, turnstiled per ZIP 318 — never the change.
        let ironwood = transaction
            .ironwood_bundle()
            .expect("the payment to an Orchard receiver routes via Ironwood post-NU6.3");
        assert_eq!(
            i64::from(*ironwood.value_balance()),
            -i64::try_from(sent_value).unwrap(),
            "Ironwood carries only the payment value, not the change"
        );
    }

    /// Gap-3 cell of the remediation plan: the entire ZIP-320 two-step
    /// builds offline behind the seam: `zcash_client_backend` chains
    /// step one's ephemeral transparent output into step two before
    /// anything touches a network. Step two's sole transparent input
    /// spends step one's ephemeral output, and the TEX-decoded P2PKH
    /// address receives the payment. The only class left live is zebra's
    /// mempool accepting the chained unmined pair.
    #[tokio::test]
    async fn tex_two_step_chains_ephemeral_output_offline() {
        use pepper_sync::keys::decode_address;
        use zcash_client_backend::address::Address;
        use zcash_client_backend::zip321::{Payment, TransactionRequest};
        use zcash_protocol::value::Zatoshis;
        use zcash_transparent::address::TransparentAddress;

        let payment_value = 100_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(5_000_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        // A TEX destination derived from an external wallet's first
        // transparent address, as ZIP 320 prescribes.
        let external_wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let taddr = external_wallet
            .transparent_addresses()
            .values()
            .next()
            .unwrap()
            .clone();
        let Address::Transparent(TransparentAddress::PublicKeyHash(taddr_bytes)) =
            decode_address(&external_wallet.chain_type(), &taddr).unwrap()
        else {
            panic!("a wallet-generated first taddr is p2pkh")
        };
        let tex_address = crate::testutils::interpret_taddr_as_tex_addr(
            taddr_bytes,
            &external_wallet.chain_type(),
        );

        let request = TransactionRequest::new(vec![Payment::without_memo(
            zcash_address::ZcashAddress::try_from_encoded(&tex_address).unwrap(),
            Zatoshis::from_u64(payment_value).unwrap(),
        )])
        .unwrap();
        let proposal = client
            .propose_send(request, zip32::AccountId::ZERO)
            .await
            .unwrap();
        let txids = client
            .wallet()
            .write()
            .await
            .calculate_transactions(proposal, zip32::AccountId::ZERO, None)
            .await
            .unwrap();
        assert_eq!(txids.len(), 2, "the ZIP-320 pair builds as two steps");

        let wallet = client.wallet();
        let wallet = wallet.read().await;
        let step_one = wallet.wallet_transactions.get(&txids[0]).unwrap();
        let step_two = wallet.wallet_transactions.get(&txids[1]).unwrap();

        let step_two_transparent = step_two
            .transaction()
            .transparent_bundle()
            .expect("the transparent leg carries a transparent bundle");
        assert_eq!(step_two_transparent.vin.len(), 1);
        let prevout = step_two_transparent.vin[0].prevout();
        assert_eq!(
            *prevout.txid(),
            txids[0],
            "step two's sole input spends step one"
        );
        let step_one_bundle = step_one
            .transaction()
            .transparent_bundle()
            .expect("the shield leg pays out an ephemeral transparent output");
        let ephemeral_output = step_one_bundle
            .vout
            .get(prevout.n() as usize)
            .expect("the spent index exists among step one's outputs");
        assert_eq!(
            step_two_transparent.vout.len(),
            1,
            "the transparent leg pays the TEX destination and nothing else"
        );
        // One transparent input, one transparent output: the ZIP-317 fee
        // is the two-action grace minimum, 10_000 zats.
        assert_eq!(
            u64::from(ephemeral_output.value()),
            payment_value + 10_000,
            "step one's ephemeral output funds step two's payment plus its fee exactly"
        );

        let expected_script: zcash_transparent::address::Script =
            TransparentAddress::PublicKeyHash(taddr_bytes)
                .script()
                .into();
        let tex_payment = step_two_transparent
            .vout
            .iter()
            .find(|out| *out.script_pubkey() == expected_script)
            .expect("one of step two's outputs pays the TEX-decoded p2pkh");
        assert_eq!(u64::from(tex_payment.value()), payment_value);
    }

    /// The boundary cell the tip_spend_rejection attribution isolated:
    /// a wallet synced to activation − 1 builds a transaction targeting
    /// the activation height, so it must commit to the POST-activation
    /// branch id. This is the permanent unit fence for the wallet-side
    /// wrong-branch-id failure observed live at the height-5 NU6.1/6.2
    /// co-activation.
    #[tokio::test]
    async fn boundary_adjacent_build_uses_post_activation_branch_id() {
        let boundary = 10;
        let heights = ActivationHeights::builder()
            .set_overwinter(Some(1))
            .set_sapling(Some(1))
            .set_blossom(Some(1))
            .set_heartwood(Some(1))
            .set_canopy(Some(1))
            .set_nu5(Some(1))
            .set_nu6(Some(1))
            .set_nu6_1(Some(1))
            .set_nu6_2(Some(boundary))
            .set_nu6_3(None)
            .set_nu7(None)
            .build();
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .tip(boundary - 1)
            .activation_heights(heights)
            .build();
        let chain = wallet.chain_type();
        let pre_activation = BranchId::for_height(&chain, BlockHeight::from_u32(boundary - 1));
        let post_activation = BranchId::for_height(&chain, BlockHeight::from_u32(boundary));
        assert_ne!(
            pre_activation, post_activation,
            "the cell must sit on a real branch boundary"
        );

        let (target, _, branch_id) = build_one_send(wallet).await;

        assert_eq!(target, boundary);
        assert_eq!(branch_id, post_activation);
    }
}

#[cfg(test)]
mod test {
    //! all tests below (and in this mod) use example wallets, which describe real-world chains.

    use zingo_test_vectors::seeds;

    use crate::{
        config::{ClientConfig, WalletConfig},
        lightclient::{LightClient, error::LightClientError, sync::test::sync_example_wallet},
        mocks::proposal::ProposalBuilder,
        testutils::{
            chain_generics::{
                conduct_chain::ConductChain as _, networked::NetworkedTestEnvironment,
                with_assertions,
            },
            default_test_wallet_settings,
        },
        wallet::disk::testing::examples,
    };

    async fn create_basic_client() -> LightClient {
        let config = ClientConfig::builder()
            .set_wallet_config(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 419200,
                wallet_settings: default_test_wallet_settings(),
            })
            .build()
            .unwrap();
        LightClient::new(config, true).await.unwrap()
    }

    #[tokio::test]
    async fn complete_and_transmit_unconnected_error() {
        let mut lc = create_basic_client().await;
        let proposal = ProposalBuilder::default().build();
        let err = lc.send(proposal, zip32::AccountId::ZERO).await.unwrap_err();
        assert!(matches!(err, LightClientError::Offline));
    }

    /// live sync: execution time increases linearly until example wallet is upgraded
    /// live send TESTNET: these assume the wallet has on-chain TAZ.
    /// waits up to five blocks for confirmation per transaction. see [`zingolib/src/testutils/chain_generics/live_chain.rs`]
    /// as of now, average block time is supposedly about 75 seconds
    mod testnet {
        use zcash_protocol::{PoolType, ShieldedPool};

        use crate::testutils::lightclient::get_base_address;

        use super::*;

        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_orchard_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;

            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 20_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();
        }
        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_sapling_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedPool::Sapling)).await;

            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 20_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();
        }
        #[ignore = "only one test can be run per testnet wallet at a time"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        /// about 273 seconds
        async fn testnet_send_to_self_transparent_and_then_shield_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;

            let client_addr = get_base_address(&client, PoolType::Transparent).await;

            let environment = &mut NetworkedTestEnvironment::setup().await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                environment,
                &mut client,
                vec![(&client_addr, 100_001, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let _ =
                with_assertions::assure_propose_shield_bump_sync(environment, &mut client, true)
                    .await
                    .unwrap();
        }
        #[ignore = "this needs to pass CI, but we arent there with testnet"]
        #[tokio::test]
        /// this is a networked sync test. its execution time scales linearly since last updated
        /// this is a networked send test. whether it can work depends on the state of live wallet on the blockchain
        async fn testnet_send_to_self_all_pools_glory_goddess() {
            let case =
                examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);

            let mut client = sync_example_wallet(case).await;
            let environment = &mut NetworkedTestEnvironment::setup().await;

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedPool::Orchard)).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 14_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let client_addr =
                get_base_address(&client, PoolType::Shielded(ShieldedPool::Sapling)).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                &mut NetworkedTestEnvironment::setup().await,
                &mut client,
                vec![(&client_addr, 15_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let client_addr = get_base_address(&client, PoolType::Transparent).await;
            with_assertions::assure_propose_send_bump_sync_all_recipients(
                environment,
                &mut client,
                vec![(&client_addr, 100_000, None)],
                vec![],
                true,
            )
            .await
            .unwrap();

            let _ =
                with_assertions::assure_propose_shield_bump_sync(environment, &mut client, true)
                    .await
                    .unwrap();
        }
    }
}

/// Migrated from libtonode `slow::t_incoming_t_outgoing_disallowed`: a
/// received transparent coin appears in the transaction summaries with its
/// height and value, and spending transparent funds through an ordinary
/// send is refused, since the wallet demands a shield first, surfacing as an
/// insufficient-funds proposal error because transparent coins are not
/// send-spendable.
#[cfg(test)]
mod transparent_policy {
    use crate::{
        lightclient::LightClient,
        lightclient::error::{LightClientError, SendError},
        testutils::lightclient::from_inputs,
        testutils::synthetic_wallet::SyntheticWalletBuilder,
        wallet::error::ProposeSendError,
    };

    #[tokio::test]
    async fn t_incoming_t_outgoing_disallowed() {
        let value = 100_000;
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(value)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let transaction = client
            .wallet()
            .read()
            .await
            .transaction_summaries(false)
            .await
            .unwrap()
            .0
            .first()
            .unwrap()
            .clone();
        // The builder confirms its first fabricated record at height 2.
        assert_eq!(transaction.blockheight, 2.into());
        assert_eq!(transaction.value, value);

        let sent_value = 20_000;
        let sent_transaction_error = from_inputs::quick_send(
            &mut client,
            vec![(zingo_test_vectors::EXT_TADDR, sent_value, None)],
        )
        .await
        .unwrap_err();
        assert!(matches!(
            sent_transaction_error,
            LightClientError::SendError(SendError::ProposeSendError(ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available: _,
                    required: _
                }
            )))
        ));
    }
}

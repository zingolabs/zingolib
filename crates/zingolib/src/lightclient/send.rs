//! TODO: Add Mod Description Here!

use std::convert::Infallible;
use std::future::Future;

use nonempty::NonEmpty;

use zcash_client_backend::data_api::wallet::TargetHeight;
use zcash_client_backend::proposal::{Proposal, ProposalError};
use zcash_client_backend::zip321::TransactionRequest;
use zcash_primitives::transaction::{TxId, fees::zip317};
use zcash_protocol::consensus::{BlockHeight, BranchId};
use zl_common::status::ConfirmationStatus;
use zl_netutils::Indexer as _;
use zl_netutils::lightwallet_protocol::{RawTransaction, TxFilter};

use crate::config::ChainType;
use crate::data::proposal::{OpReturnProposal, ZingoProposal};
use crate::lightclient::error::{LightClientError, SendError, TransmissionError};
use crate::lightclient::indexer_history::{
    AttemptKind, AttemptRoute, FailureKind, FaultDomain, IndexerAttempt, IndexerHistoryHandle,
    fault_domain, now_unix_secs,
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
    outcome: &Result<String, zl_netutils::diag::NetOpFailure>,
    fault_domain: Option<FaultDomain>,
) {
    history.record(&IndexerAttempt {
        unix_secs: now_unix_secs(),
        host: host.clone(),
        route,
        kind: AttemptKind::Send,
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        fault_domain,
        outcome: match outcome {
            Ok(_) => Ok(()),
            Err(failure) => Err(FailureKind::classify(&failure.to_string())),
        },
    });
}

#[derive(Debug, thiserror::Error)]
#[error("{host}: {failure}")]
pub(crate) struct TransmitError {
    host: String,
    failure: zl_netutils::diag::NetOpFailure,
}

use crate::lightclient::{DEFAULT_REQUEST_TIMEOUT, LightClient};
use crate::wallet::LightWallet;
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
    /// confirmation, retries included.
    pub round_trip: std::time::Duration,
}

#[cfg(feature = "nym")]
pub(crate) fn resolve_transmit_route(
    has_indexer: bool,
    route: Result<crate::mixnet::MixnetRoute, crate::mixnet::MixnetNotReady>,
) -> Result<Option<zl_netutils::conduit::ConduitDial>, LightClientError> {
    use crate::mixnet::{MixnetNotReady, MixnetRoute};
    match (has_indexer, route) {
        // The guard rides out to the caller, which holds it for the send.
        (true, Ok(MixnetRoute::Mixnet(conduit))) => Ok(Some(conduit.dial())),
        (true, Ok(MixnetRoute::Nakednet)) => Ok(None),
        (false, Ok(_)) => Err(LightClientError::Offline),
        (false, Err(MixnetNotReady::Unattached)) => Err(LightClientError::Offline),
        (_, Err(e)) => Err(LightClientError::MixnetNotReady(e)),
    }
}

/// The route one transmitted transaction traveled (ADR 0011).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransmitRoute {
    /// Direct submission to the Session Indexer.
    Nakednet {
        /// The Session Indexer's host.
        destination: String,
    },
    /// The Session Indexer reached through the local SOCKS5 tunnel
    /// endpoint.
    Mixnet {
        /// The Session Indexer's host.
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
/// the delta of [`crate::wallet::expiry::tx_expiry_delta`], so the lift gives
/// it the longest expiry the epoch permits: it stays transmittable until the
/// next scheduled network upgrade. That is the outer limit for any pre-signed Zcash transaction:
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
        _ => {
            let highest_encodable = ZIP_203_EXPIRY_HEIGHT_THRESHOLD - 1;
            highest_encodable
                - crate::wallet::expiry::tx_expiry_delta(
                    chain_type,
                    BlockHeight::from_u32(highest_encodable),
                )
        }
    };
    let lifted_target = cap.max(u32::from(stored_target));
    Proposal::multi_step(
        proposal.fee_rule().clone(),
        TargetHeight::from(lifted_target),
        proposal.confirmations_policy(),
        proposal.steps().clone(),
    )
}

/// A gRPC indexer as a [`TransmitTarget`].
struct NakednetTarget(zl_netutils::GrpcIndexer);

impl NakednetTarget {
    fn failure(
        status: &zl_netutils::Status,
        host: &crate::destination::Host,
    ) -> zl_netutils::diag::NetOpFailure {
        zl_netutils::diag::NetOpFailure::from_error(
            zl_netutils::diag::NetOpStage::RemoteHttp,
            host,
            status,
        )
    }
}

impl TransmitTarget for NakednetTarget {
    type Failure = zl_netutils::Status;

    fn submit(
        &self,
        raw_tx: &[u8],
        height: u64,
    ) -> impl Future<Output = Result<String, zl_netutils::Status>> + Send {
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

/// A [`zl_netutils::Socks5Indexer`] is the mixnet [`TransmitTarget`]:
/// one Destination that submits and delivery-checks over its own tunnel,
/// running the same [`resilient_transmit`] policy as the nakednet wire.
#[cfg(feature = "nym")]
impl TransmitTarget for zl_netutils::Socks5Indexer {
    type Failure = zl_netutils::Socks5TransmitError;

    async fn submit(
        &self,
        raw_tx: &[u8],
        height: u64,
    ) -> Result<String, zl_netutils::Socks5TransmitError> {
        self.send_transaction(raw_tx, height).await
    }

    fn knows_transaction(&self, txid: &TxId) -> impl Future<Output = bool> + Send {
        let hash = txid.as_ref().to_vec();
        async move { self.transaction_known(&hash).await }
    }
}

/// The wire one Transmission's pulls travel.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Wire {
    /// Direct connections.
    Nakednet,
    /// The session's standing mixnet client.
    #[cfg(feature = "nym")]
    Mixnet { shared_socks5: std::net::SocketAddr },
    /// A test-attached slot whose arms submit over the mock indexer's channel.
    #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
    MixnetOverMock { shared_socks5: std::net::SocketAddr },
}

impl Wire {
    fn attempt_route(self) -> AttemptRoute {
        match self {
            Wire::Nakednet => AttemptRoute::Nakednet,
            #[cfg(feature = "nym")]
            Wire::Mixnet { .. } => AttemptRoute::Mixnet,
            #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
            Wire::MixnetOverMock { .. } => AttemptRoute::Mixnet,
        }
    }

    #[cfg_attr(not(feature = "nym"), allow(dead_code))]
    fn is_mixnet(self) -> bool {
        !matches!(self, Wire::Nakednet)
    }

    fn route_to(self, destination: &crate::destination::Host) -> TransmitRoute {
        match self {
            Wire::Nakednet => TransmitRoute::Nakednet {
                destination: destination.to_string(),
            },
            #[cfg(feature = "nym")]
            Wire::Mixnet { shared_socks5 } => TransmitRoute::Mixnet {
                destination: destination.to_string(),
                via_socks5: shared_socks5.to_string(),
            },
            #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
            Wire::MixnetOverMock { shared_socks5 } => TransmitRoute::Mixnet {
                destination: destination.to_string(),
                via_socks5: shared_socks5.to_string(),
            },
        }
    }
}

/// The ambient state a transmission narrates through, records against, and paces itself by.
struct TransmitContext<'a> {
    progress: &'a TransmitProgressHandle,
    history: &'a IndexerHistoryHandle,
    retry_interval: std::time::Duration,
}

enum SessionTarget {
    Nakednet(NakednetTarget),
    #[cfg(feature = "nym")]
    Mixnet(zl_netutils::Socks5Indexer),
}

impl SessionTarget {
    #[cfg_attr(not(feature = "nym"), allow(unused_variables))]
    fn of(wire: Wire, indexer: zl_netutils::GrpcIndexer, indexer_uri: http::Uri) -> Self {
        match wire {
            Wire::Nakednet => SessionTarget::Nakednet(NakednetTarget(indexer)),
            #[cfg(feature = "nym")]
            Wire::Mixnet { shared_socks5 } => {
                SessionTarget::Mixnet(zl_netutils::Socks5Indexer::new(
                    shared_socks5,
                    indexer_uri,
                    DEFAULT_REQUEST_TIMEOUT,
                ))
            }
            #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
            Wire::MixnetOverMock { .. } => SessionTarget::Nakednet(NakednetTarget(indexer)),
        }
    }
}

/// - Records one attempt against the Session Indexer in the session's history.
async fn transmit_one_transaction(
    wire: Wire,
    target: &SessionTarget,
    host: &crate::destination::Host,
    tx_bytes: &[u8],
    height: u64,
    txid: &TxId,
    context: &TransmitContext<'_>,
) -> Result<(String, TransmitRoute), TransmitError> {
    match target {
        SessionTarget::Nakednet(target) => {
            transmit_to(
                wire,
                host,
                target,
                NakednetTarget::failure,
                tx_bytes,
                height,
                txid,
                context,
            )
            .await
        }
        #[cfg(feature = "nym")]
        SessionTarget::Mixnet(target) => {
            transmit_to(
                wire,
                host,
                target,
                |error, host| crate::mixnet::socks5_transmit_failure(error, host),
                tx_bytes,
                height,
                txid,
                context,
            )
            .await
        }
    }
}

/// - Records the attempt in the session's indexer history.
#[allow(clippy::too_many_arguments)]
async fn transmit_to<T, F>(
    wire: Wire,
    host: &crate::destination::Host,
    target: &T,
    describe_failure: F,
    tx_bytes: &[u8],
    height: u64,
    txid: &TxId,
    context: &TransmitContext<'_>,
) -> Result<(String, TransmitRoute), TransmitError>
where
    T: TransmitTarget + Sync,
    F: Fn(&T::Failure, &crate::destination::Host) -> zl_netutils::diag::NetOpFailure,
{
    let started = std::time::Instant::now();
    let outcome = resilient_transmit(
        target,
        tx_bytes,
        height,
        txid,
        move |_| tokio::time::sleep(context.retry_interval),
        |event| context.progress.set(format!("destination {host}: {event}")),
    )
    .await
    .map_err(|TransmitFailed(failure)| describe_failure(&failure, host));
    record_send_attempt(
        context.history,
        host,
        wire.attempt_route(),
        started,
        &outcome,
        outcome
            .as_ref()
            .err()
            .map(|failure| fault_domain(&failure.stage)),
    );
    match outcome {
        Ok(server_txid) => Ok((server_txid, wire.route_to(host))),
        Err(failure) => Err(TransmitError {
            host: host.to_string(),
            failure,
        }),
    }
}

fn fail_unsent(wallet: &mut LightWallet, calculated_txids: &NonEmpty<TxId>, from: usize) {
    let unsent: Vec<TxId> = calculated_txids.iter().skip(from).copied().collect();
    pepper_sync::set_transactions_failed(&mut wallet.wallet_transactions, unsent);
    wallet.truncate_failed_refund_addresses();
    wallet.save_required = true;
}

impl LightClient {
    async fn send(
        &mut self,
        proposal: Proposal<zip317::FeeRule, OutputRef>,
        sending_account: zip32::AccountId,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        self.preflight_transmit()?;
        let indexerless = !self.network.is_online();
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
            .calculate_transactions(proposal, sending_account)
            .await
            .map_err(|e| {
                wallet.truncate_refund_addresses(highest_refund_address_index);

                SendError::CalculateSendError(e)
            })?;
        drop(wallet);

        self.transmit_transactions(calculated_txids).await
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
            .calculate_transactions(proposal, shielding_account)
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
                ZingoProposal::OpReturn(proposal) => self.send_op_return(proposal).await,
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
    /// [`crate::wallet::expiry::tx_expiry_delta`] blocks past the target:
    /// connected callers are expected to transmit promptly.
    pub async fn calculate_stored_proposal(&mut self) -> Result<NonEmpty<TxId>, LightClientError> {
        let indexerless = !self.network.is_online();
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
                        .calculate_transactions(proposal, sending_account)
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
                        .calculate_transactions(proposal, shielding_account)
                        .await
                        .map_err(SendError::CalculateShieldError),
                    Err(e) => Err(e),
                }
            }
            ZingoProposal::OpReturn(proposal) => {
                wallet.store_proposal(ZingoProposal::OpReturn(proposal));
                return Err(SendError::OpReturnNotCalculable.into());
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
    /// [`Self::transmit_transactions`] performs for real: nakednet demands
    /// the configured indexer, and an Indexerless session passes only with
    /// a ready mixnet (ruling 2026-07-29).
    fn preflight_transmit(&self) -> Result<(), LightClientError> {
        #[cfg(feature = "nym")]
        {
            resolve_transmit_route(self.network.is_online(), self.send_route()).map(|_| ())
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

    /// Proposes and transmits an OP_RETURN send skipping proposal
    /// confirmation. See [`Self::propose_send_with_op_return`] for the
    /// proposal and [`OpReturnProposal`] for the two transactions.
    ///
    /// If sync is running, it is paused before creating the proposal. If
    /// `resume_sync` is `true`, the engine is restored to its prior mode
    /// after the send. If `false`, it stays paused for the caller to
    /// resume.
    ///
    /// Returns the transmit reports for both transactions, deshield first.
    pub async fn quick_send_with_op_return(
        &mut self,
        recipient: &str,
        amount: zcash_protocol::value::Zatoshis,
        data: crate::wallet::transparent::OpReturnData,
        account_id: zip32::AccountId,
        resume_sync: bool,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        let guard = self.pause_sync_scoped().ok();
        let reports = match self
            .create_op_return_proposal(recipient, amount, data, account_id)
            .await
        {
            Ok(proposal) => self.send_op_return(proposal).await,
            Err(e) => Err(e),
        };
        if let Some(guard) = guard
            && !resume_sync
        {
            guard.disarm();
        }

        reports
    }

    /// Reserves the source address of `proposal`, transmits its deshield,
    /// then builds and transmits the OP_RETURN send that spends it.
    ///
    /// The source address must still be the next unreserved Refund-scope
    /// address. If another send reserved it after the proposal was made,
    /// the proposal is stale and is refused. A deshield that fails before
    /// transmission releases the reservation.
    ///
    /// If a step fails after the deshield is transmitted, the error is
    /// [`SendError::OpReturnAfterDeshield`]. It names the deshield txid.
    /// If the OP_RETURN transaction was not built, the proposal is stored
    /// again with that txid and the next `send_stored_proposal` resumes
    /// from the OP_RETURN step. If it was built, it stays in the wallet
    /// with `Calculated` status and the error names its txid.
    ///
    /// A proposal that carries a deshield txid skips the reservation and
    /// the deshield.
    ///
    /// Returns the transmit reports for both transactions, deshield first.
    /// A resumed send returns the OP_RETURN report only.
    async fn send_op_return(
        &mut self,
        proposal: OpReturnProposal,
    ) -> Result<NonEmpty<TransmitReport>, LightClientError> {
        let account = proposal.sending_account();

        let (deshield_reports, deshield_txid) = match proposal.deshield_txid() {
            Some(txid) => (None, txid),
            None => {
                let highest_before = {
                    let mut wallet = self.wallet().write().await;
                    let (next_id, next_address) = wallet
                        .derive_refund_addresses(1, account)
                        .map_err(|e| SendError::OpReturn(WalletError::from(e)))?[0];
                    if next_id != proposal.source_address_id()
                        || next_address != *proposal.source_address()
                    {
                        return Err(SendError::OpReturnSourceAddressStale.into());
                    }
                    let highest_before = wallet.highest_refund_address_index();
                    wallet
                        .generate_refund_addresses(1, account)
                        .map_err(|e| SendError::OpReturn(WalletError::from(e)))?;
                    highest_before
                };

                match self.send(proposal.deshield().clone(), account).await {
                    Ok(reports) => {
                        let txid = reports.first().txid;
                        (Some(reports), txid)
                    }
                    Err(e) => {
                        let transmitted = matches!(
                            e,
                            LightClientError::SendError(SendError::TransmissionError(_))
                        );
                        if !transmitted {
                            self.wallet()
                                .write()
                                .await
                                .truncate_refund_addresses(highest_before);
                        }
                        return Err(e);
                    }
                }
            }
        };

        let op_return_txid = {
            let mut wallet = self.wallet().write().await;
            let built = wallet
                .get_migration_heights()
                .map_err(SendError::OpReturn)
                .and_then(|heights| {
                    heights
                        .map(|(target, _)| target)
                        .ok_or(SendError::OpReturn(WalletError::NoSyncData))
                })
                .and_then(|target_height| {
                    wallet
                        .find_transparent_output(deshield_txid, proposal.source_address())
                        .map(|(outpoint, txout)| (outpoint, txout, target_height))
                        .map_err(SendError::OpReturn)
                })
                .and_then(|(source_outpoint, source_txout, target_height)| {
                    wallet
                        .build_op_return_send(
                            account,
                            proposal.source_address_id(),
                            source_outpoint,
                            source_txout,
                            proposal.recipient(),
                            proposal.amount(),
                            proposal.data(),
                            target_height,
                        )
                        .map_err(SendError::OpReturn)
                });
            match built {
                Ok(txid) => txid,
                Err(e) => {
                    wallet.store_proposal(ZingoProposal::OpReturn(
                        proposal.with_deshield_txid(deshield_txid),
                    ));
                    return Err(SendError::OpReturnAfterDeshield {
                        deshield_txid,
                        op_return_txid: None,
                        source: Box::new(e.into()),
                    }
                    .into());
                }
            }
        };

        let op_return_reports = self
            .transmit_transactions(NonEmpty::new(op_return_txid))
            .await
            .map_err(|e| SendError::OpReturnAfterDeshield {
                deshield_txid,
                op_return_txid: Some(op_return_txid),
                source: Box::new(e),
            })?;

        Ok(match deshield_reports {
            Some(mut reports) => {
                for report in op_return_reports {
                    reports.push(report);
                }
                reports
            }
            None => op_return_reports,
        })
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
        let connected = self
            .network
            .connected()
            .map(|(uri, indexer)| (uri.clone(), indexer.clone()));

        // Resolve the send route once for the whole send (ADR 0011), under
        // the session's transmit policy as it stands at this moment.
        // The guard is bound for the whole send, so the conduit counts this
        // transmission as outstanding until the last transaction is delivered.
        #[cfg(feature = "nym")]
        let transmit_dial = resolve_transmit_route(connected.is_some(), self.send_route())?;
        // A test-attached slot pairs its mixnet route with arms that submit
        // over the mock indexer's channel; a live Ready session keeps the
        // SOCKS5 tunnel, and a nakednet-policy send over a test slot
        // takes the nakednet arm as it would in production. Production
        // builds carry no test slot state, so this distinction does not
        // exist there.
        #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
        let mock_arms = matches!(
            *self.mixnet_slot.lock().expect("mixnet slot mutex"),
            crate::mixnet::MixnetSlot::AttachedForTests {
                mock_arms: true,
                ..
            }
        );
        #[cfg(feature = "nym")]
        let wire = match transmit_dial.as_ref().map(|dial| dial.socks5()) {
            None => Wire::Nakednet,
            #[cfg(any(test, feature = "testutils"))]
            Some(shared_socks5) if mock_arms => Wire::MixnetOverMock { shared_socks5 },
            Some(shared_socks5) => Wire::Mixnet { shared_socks5 },
        };
        #[cfg(not(feature = "nym"))]
        let wire = Wire::Nakednet;
        // Every wire reaches the Session Indexer (zingo-adrs zingolib/0063).
        let (indexer_uri, indexer) = connected.ok_or(LightClientError::Offline)?;
        let host = crate::destination::Host::of_uri(&indexer_uri);
        // One target serves the whole send: the session's live indexer
        // client on nakednet, one SOCKS5 client over the mixnet.
        let target = SessionTarget::of(wire, indexer, indexer_uri);

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
                    fail_unsent(&mut wallet, &calculated_txids, index);
                    WalletError::TransactionWrite(e)
                })?;

            let dispatched = std::time::Instant::now();
            let transmit_context = TransmitContext {
                progress: &progress,
                history: &history,
                retry_interval: self.transmit_retry_interval,
            };
            let transmit_outcome = transmit_one_transaction(
                wire,
                &target,
                &host,
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
                    if wire.is_mixnet() {
                        self.note_standing_exit_suspicion();
                    }
                    fail_unsent(&mut wallet, &calculated_txids, index);
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
mod transmit_error_seam;

#[cfg(test)]
mod built_transaction_shape;

#[cfg(test)]
mod test;

/// Migrated from libtonode `slow::t_incoming_t_outgoing_disallowed`: a
/// received transparent coin appears in the transaction summaries with its
/// height and value, and spending transparent funds through an ordinary
/// send is refused, since the wallet demands a shield first, surfacing as an
/// insufficient-funds proposal error because transparent coins are not
/// send-spendable.
#[cfg(test)]
mod transparent_policy;

#[cfg(test)]
mod op_return;

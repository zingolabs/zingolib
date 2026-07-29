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
    host: &str,
    route: AttemptRoute,
    started: std::time::Instant,
    outcome: &Result<String, String>,
) {
    history.record(&IndexerAttempt {
        unix_secs: now_unix_secs(),
        host: host.to_string(),
        route,
        kind: AttemptKind::Send,
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        outcome: match outcome {
            Ok(_) => Ok(()),
            Err(detail) => Err(FailureKind::classify(detail)),
        },
    });
}
use crate::lightclient::{DEFAULT_REQUEST_TIMEOUT, LightClient};
use crate::wallet::error::WalletError;
use crate::wallet::output::OutputRef;

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

/// A single Broadcast Indexer reached through the local SOCKS5 proxy, as a
/// [`TransmitTarget`]: it submits and delivery-checks over the mixnet tunnel,
/// running the same [`resilient_transmit`] policy as the clearnet path. The
/// fan-out builds one of these per pick.
#[cfg(feature = "nym")]
struct SocksTarget {
    socks5_addr: String,
    indexer: http::Uri,
}

#[cfg(feature = "nym")]
impl TransmitTarget for SocksTarget {
    type Failure = zingo_netutils::Socks5TransmitError;

    fn submit(
        &self,
        raw_tx: &[u8],
        height: u64,
    ) -> impl Future<Output = Result<String, zingo_netutils::Socks5TransmitError>> + Send {
        let socks5_addr = self.socks5_addr.clone();
        let indexer = self.indexer.clone();
        let data = raw_tx.to_vec();
        async move {
            zingo_netutils::send_transaction_via_socks5(
                &socks5_addr,
                &indexer,
                &data,
                height,
                DEFAULT_REQUEST_TIMEOUT,
            )
            .await
        }
    }

    fn knows_transaction(&self, txid: &TxId) -> impl Future<Output = bool> + Send {
        let socks5_addr = self.socks5_addr.clone();
        let indexer = self.indexer.clone();
        let hash = txid.as_ref().to_vec();
        async move {
            zingo_netutils::transaction_known_via_socks5(
                &socks5_addr,
                &indexer,
                &hash,
                DEFAULT_REQUEST_TIMEOUT,
            )
            .await
        }
    }
}

/// Submit one transaction under the route the Mixnet Mode policy resolved:
/// clearnet through the configured indexer when `socks5_proxy` is `None`, or
/// the mixnet fan-out over the Broadcast Indexers reached through the SOCKS5
/// proxy when it is `Some`. Returns the server-reported txid or the last
/// failure message.
async fn transmit_one_transaction(
    socks5_proxy: Option<&str>,
    indexer: &zingo_netutils::GrpcIndexer,
    tx_bytes: &[u8],
    height: u64,
    txid: &TxId,
    progress: &TransmitProgressHandle,
    history: &IndexerHistoryHandle,
) -> Result<String, String> {
    match socks5_proxy {
        None => {
            let host = indexer
                .uri()
                .host()
                .map_or_else(|| indexer.uri().to_string(), str::to_string);
            let started = std::time::Instant::now();
            // The typed status is rendered only at this boundary, which is
            // the send path's existing prose seam (the NotYetTyped backlog);
            // below it the failure travels whole.
            let outcome = resilient_transmit(
                &ClearnetTarget(indexer.clone()),
                tx_bytes,
                height,
                txid,
                |interval| tokio::time::sleep(interval),
                |event| progress.set(format!("indexer {host}: {event}")),
            )
            .await
            .map_err(|TransmitFailed(status)| status.to_string());
            record_send_attempt(history, &host, AttemptRoute::Clearnet, started, &outcome);
            outcome
        }
        #[cfg(feature = "nym")]
        Some(socks5_addr) => {
            mixnet_fanout_transmit(
                socks5_addr,
                indexer.uri(),
                tx_bytes,
                height,
                txid,
                progress,
                history,
            )
            .await
        }
        #[cfg(not(feature = "nym"))]
        Some(_) => Err("a mixnet route requires the nym feature".to_string()),
    }
}

/// Broadcast one transaction over the mixnet as the escalating, serially gated
/// fan-out (ADR 0011): each arm runs the shared [`resilient_transmit`] policy
/// against one Broadcast Indexer through the SOCKS5 proxy, and the fan-out
/// escalates round by round until an indexer confirms delivery or the witness
/// cap is reached.
///
/// The draw comes from [`eligible_witnesses`], never the raw curated list: a
/// witness is never the sync indexer's operator (ADR 0022), because that party
/// already holds the wallet's address set and must not receive the broadcast
/// too. An emptied pool refuses rather than falling back.
#[cfg(feature = "nym")]
async fn mixnet_fanout_transmit(
    socks5_addr: &str,
    sync_indexer: &http::Uri,
    tx_bytes: &[u8],
    height: u64,
    txid: &TxId,
    progress: &TransmitProgressHandle,
    history: &IndexerHistoryHandle,
) -> Result<String, String> {
    use crate::nym::broadcast::{MAX_BROADCAST_WITNESSES, fanout_broadcast};
    use crate::nym::broadcast_indexers::eligible_witnesses;

    let indexers = eligible_witnesses(sync_indexer).map_err(|e| e.to_string())?;
    let run_arm = |indexer: http::Uri| {
        let socks5_addr = socks5_addr.to_string();
        let tx_bytes = tx_bytes.to_vec();
        let txid = *txid;
        let host = indexer
            .host()
            .map_or_else(|| indexer.to_string(), str::to_string);
        async move {
            let target = SocksTarget {
                socks5_addr,
                indexer,
            };
            let started = std::time::Instant::now();
            // The arm's failure becomes the taxonomy record — stage by typed
            // match, cause chain captured layer by layer, target the witness
            // host — which the fan-out collects whole per witness.
            let outcome = resilient_transmit(
                &target,
                &tx_bytes,
                height,
                &txid,
                |interval| tokio::time::sleep(interval),
                |event| progress.set(format!("witness {host}: {event}")),
            )
            .await
            .map_err(|TransmitFailed(error)| crate::nym::socks5_transmit_failure(&error, &host));
            let rendered = outcome.clone().map_err(|failure| failure.to_string());
            record_send_attempt(history, &host, AttemptRoute::Mixnet, started, &rendered);
            outcome
        }
    };

    fanout_broadcast(
        &indexers,
        &mut rand::rngs::OsRng,
        MAX_BROADCAST_WITNESSES,
        run_arm,
        |line| progress.set(format!("mixnet fan-out: {line}")),
    )
    .await
    .map_err(|error| error.to_string())
}

/// The chain-mock twin of [`mixnet_fanout_transmit`], paired with the
/// test-attached slot state behind
/// [`LightClient::switch_on_mixnet_for_tests`]: the witness draw, the
/// escalation rounds, and the cap run for real over the curated Broadcast
/// Indexer pool, while each arm's bytes travel the mock indexer's channel
/// instead of a SOCKS5 tunnel. The tunnel's byte transport is pinned by
/// zingo-netutils' own tests, so no packet leaves the process here.
#[cfg(all(feature = "nym", any(test, feature = "testutils")))]
async fn mock_fanout_transmit(
    indexer: &zingo_netutils::GrpcIndexer,
    tx_bytes: &[u8],
    height: u64,
    txid: &TxId,
    progress: &TransmitProgressHandle,
    history: &IndexerHistoryHandle,
) -> Result<String, String> {
    use crate::nym::broadcast::{MAX_BROADCAST_WITNESSES, fanout_broadcast};
    use crate::nym::broadcast_indexers::eligible_witnesses;

    let witnesses = eligible_witnesses(indexer.uri()).map_err(|e| e.to_string())?;
    let run_arm = |witness: http::Uri| {
        let target = ClearnetTarget(indexer.clone());
        let tx_bytes = tx_bytes.to_vec();
        let txid = *txid;
        let host = witness
            .host()
            .map_or_else(|| witness.to_string(), str::to_string);
        async move {
            let started = std::time::Instant::now();
            let outcome = resilient_transmit(
                &target,
                &tx_bytes,
                height,
                &txid,
                |interval| tokio::time::sleep(interval),
                |event| progress.set(format!("witness {host}: {event}")),
            )
            .await
            .map_err(|TransmitFailed(status)| status.to_string());
            record_send_attempt(history, &host, AttemptRoute::Mixnet, started, &outcome);
            outcome
        }
    };

    fanout_broadcast(
        &witnesses,
        &mut rand::rngs::OsRng,
        MAX_BROADCAST_WITNESSES,
        run_arm,
        |line| progress.set(format!("mixnet fan-out: {line}")),
    )
    .await
    .map_err(|error| error.to_string())
}

impl LightClient {
    async fn send(
        &mut self,
        proposal: Proposal<zip317::FeeRule, OutputRef>,
        sending_account: zip32::AccountId,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        self.require_indexer()?;
        let mut wallet = self.wallet().write().await;
        let highest_refund_address_index = wallet.highest_refund_address_index();
        let calculated_txids = wallet
            .calculate_transactions(proposal, sending_account)
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
    ) -> Result<NonEmpty<TxId>, LightClientError> {
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
            let txids = match proposal {
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

            txids
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
        };
        drop(wallet);
        // The proposal is consumed on every path above, so its pause
        // has nothing left to guard; the engine returns to its prior mode.
        self.release_proposal_pause(true);
        result.map_err(LightClientError::from)
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
        self.transmit_transactions(calculated_txids).await
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
        // Proposing is an Indexerless capability; only the calculate/transmit
        // stage below demands a connection.
        let guard = self.pause_sync_scoped().ok();
        let proposal_result = self
            .wallet()
            .write()
            .await
            .create_send_proposal(request, account_id)
            .map_err(SendError::ProposeSendError);
        let txids = match proposal_result {
            Ok(proposal) => self.send(proposal, account_id).await,
            Err(e) => Err(e.into()),
        };
        if let Some(guard) = guard
            && !resume_sync
        {
            guard.disarm();
        }

        txids
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

        self.shield(proposal, account_id).await
    }

    /// Tranmits calculated transactions stored in the wallet matching txids of `calculated_txids` in the given order.
    /// Returns list of txids for successfully transmitted transactions.
    pub(crate) async fn transmit_transactions(
        &mut self,
        calculated_txids: NonEmpty<TxId>,
    ) -> Result<NonEmpty<TxId>, LightClientError> {
        let indexer = self.require_indexer()?.clone();

        // Resolve the Mixnet Mode route once for the whole send (ADR 0011).
        // `Clearnet` submits through the configured indexer; `Mixnet(addr)`
        // routes the fan-out through the SOCKS5 proxy; `Bootstrapping` fails
        // closed here, before any submission, rather than leaking to clearnet.
        // Without the `nym` feature there is no mixnet, so the route is clearnet.
        #[cfg(feature = "nym")]
        let socks5_proxy: Option<String> = match self.mixnet_route()? {
            crate::nym::MixnetRoute::Clearnet => None,
            crate::nym::MixnetRoute::Mixnet(socks5_addr) => Some(socks5_addr),
        };
        #[cfg(not(feature = "nym"))]
        let socks5_proxy: Option<String> = None;

        // A test-attached slot pairs its Ready route with arms that submit
        // over the mock indexer's channel; a live Ready session keeps the
        // SOCKS5 fan-out. Production builds carry no test slot state, so
        // this distinction does not exist there.
        #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
        let mock_arms = matches!(
            self.mixnet_slot,
            crate::nym::MixnetSlot::AttachedForTests { .. }
        );

        // Narrate the transmission into the side channel; the scope clears it
        // on every exit so no stale line outlives this call.
        let progress = self.transmit_progress.clone();
        let _progress_scope = TransmitProgressScope(progress.clone());
        let history = self.indexer_history.clone();
        let total = calculated_txids.len();

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
            // directly and the mixnet path runs it per fan-out arm. Wallet-state
            // effects stay here, around the pure transmission.
            #[cfg(all(feature = "nym", any(test, feature = "testutils")))]
            let transmit_outcome = if mock_arms {
                mock_fanout_transmit(
                    &indexer,
                    &transaction_bytes,
                    height.into(),
                    txid,
                    &progress,
                    &history,
                )
                .await
            } else {
                transmit_one_transaction(
                    socks5_proxy.as_deref(),
                    &indexer,
                    &transaction_bytes,
                    height.into(),
                    txid,
                    &progress,
                    &history,
                )
                .await
            };
            #[cfg(not(all(feature = "nym", any(test, feature = "testutils"))))]
            let transmit_outcome = transmit_one_transaction(
                socks5_proxy.as_deref(),
                &indexer,
                &transaction_bytes,
                height.into(),
                txid,
                &progress,
                &history,
            )
            .await;
            let txid_from_server = match transmit_outcome {
                Ok(server_txid) => server_txid,
                Err(message) => {
                    pepper_sync::set_transactions_failed(
                        &mut wallet.wallet_transactions,
                        vec![*txid],
                    );
                    wallet.save_required = true;
                    return Err(SendError::TransmissionError(
                        TransmissionError::TransmissionFailed(message),
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
        }

        Ok(calculated_txids)
    }
}

/// Gap-4 cells of the protection audit's remediation plan
/// (docs/testing/test-protection-audit-dev-to-ironwood.md § Gap
/// remediation plan): the built transaction's expiry and consensus
/// branch id must derive from the wallet's synced height + 1.
/// `LightWallet::calculate_transactions` is the build-without-broadcast
/// seam (it proves and stores the transaction without transmitting),
/// so these cells run offline over a synthetic wallet.
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

    /// Builds (without broadcasting) one send-all from the given wallet
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
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
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
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
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
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
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
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
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
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
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
    async fn complete_and_broadcast_unconnected_error() {
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

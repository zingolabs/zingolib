//! Errors assoicated with [`crate::lightclient::LightClient`].

use std::convert::Infallible;

use zcash_protocol::TxId;

use pepper_sync::error::{SyncError, SyncModeError};
use zingo_netutils::GetClientError;

#[cfg(feature = "nym")]
use crate::wallet::error::PriceError;
use crate::wallet::{
    error::{CalculateTransactionError, ProposeSendError, ProposeShieldError, WalletError},
    output::OutputRef,
};

#[derive(Debug, thiserror::Error)]
pub enum LightClientError {
    /// Sync failed to launch..
    #[error("Sync failed to launch.")]
    SyncLaunchError,
    /// Sync not running.
    #[error("No sync handle. Sync is not running.")]
    SyncNotRunning,
    /// Sync error.
    #[error("Sync error.")]
    SyncError(#[from] SyncError<WalletError>),
    /// Sync mode error.
    #[error("Sync mode error.")]
    SyncModeError(#[from] SyncModeError),
    /// Send error.
    #[error("Send error.")]
    SendError(#[from] SendError),
    /// gPRC client error.
    #[error("gRPC client error.")]
    ClientError(#[from] GetClientError),
    /// Indexer request error.
    #[error("Indexer request error.")]
    IndexerError(#[from] zingo_netutils::Status),
    /// File error.
    #[error("File error. {0}")]
    FileError(std::io::Error),
    /// Wallet error.
    #[error("Wallet error.")]
    WalletError(#[from] WalletError),
    /// Ironwood migration error.
    #[error("Ironwood migration error.")]
    MigrationError(#[from] MigrationError),
    /// No indexer configured. Call set_indexer_uri() to connect before calling network operations.
    #[error("Offline: no indexer configured. Call set_indexer_uri() to connect.")]
    Offline,
    /// Price fetch error. Exists only in nym builds: the fetch compiles
    /// only with the mixnet stack, so other builds have no fetch to fail.
    #[cfg(feature = "nym")]
    #[error("Price fetch error.")]
    PriceError(#[from] PriceError),
    /// A mixnet-covered surface was attempted while the mixnet was unavailable.
    #[cfg(feature = "nym")]
    #[error(transparent)]
    MixnetNotReady(#[from] crate::mixnet::MixnetNotReady),
    /// A probe target outside the one endpoint shape the mixnet exit
    /// policy carries.
    #[cfg(feature = "nym")]
    #[error("probe targets must be https on port 443; got '{0}'")]
    IneligibleProbeTarget(http::Uri),
    /// The configured migration transmission target shares the
    /// synchronization endpoint's host, which would let that server correlate
    /// the wallet's sync stream with its migration cohort (ADR 0011,
    /// 2026-07-23).
    #[error(
        "the migration transmission target '{host}' is the synchronization endpoint; migration \
         parts never go to the sync server. Configure a different migration_transmission_uri or \
         remove it to use the Destination Rotation."
    )]
    MigrationTransmissionTargetIsSyncEndpoint {
        /// The host both endpoints share.
        host: String,
    },
    /// No Destination remains to carry migration parts.
    #[error(transparent)]
    NoEligibleDestination(#[from] crate::destination::servers::NoEligibleDestinations),
}

/// Errors from the Orchard→Ironwood migration entry points
/// ([`crate::lightclient::LightClient`] methods backed by
/// [`crate::wallet::migration`]). Plain-data payloads so an FFI layer can
/// map each variant mechanically.
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    /// An operation needed a migration and none is in progress.
    #[error("No migration in progress.")]
    NoMigration,
    /// A second migration was started while one is in progress.
    #[error("A migration is already in progress.")]
    AlreadyInProgress,
    /// The plan is not the wallet's current plan.
    #[error("The plan does not match the wallet's current notes. Plan again.")]
    PlanMismatch,
    /// The schedule can change only while every transfer is unsigned.
    #[error("A transfer is signed; the schedule can no longer change.")]
    ScheduleFixed,
    /// The proposed schedule does not match the wallet's funding notes.
    #[error("The proposed schedule does not match the wallet's current notes. Propose again.")]
    ScheduleMismatch,
    /// A note-preparation round is still in flight.
    #[error("A note-preparation round is still confirming.")]
    RoundPending {
        /// The transactions of that round.
        txids: Vec<TxId>,
    },
    /// Note preparation is finished; the next step is the schedule.
    #[error("Note preparation is complete; commit a schedule.")]
    AlreadyPrepared,
    /// The command needs note preparation to be complete.
    #[error("Note preparation is not complete.")]
    NotPrepared,
    /// The command needs a committed schedule.
    #[error("No schedule is committed.")]
    NotScheduled,
    /// The wallet's view of the chain is older than one window. Sync first.
    #[error(
        "The wallet last saw the chain at height {last_known_height} more than one window ago. Sync first."
    )]
    StaleChainView {
        /// The last height the wallet scanned.
        last_known_height: zcash_protocol::consensus::BlockHeight,
    },
    /// The transfer is not pending, so it cannot be released.
    #[error("Transfer {0} is not pending.")]
    TransferNotPending(u32),
    /// The plan is of the other migration mode.
    #[error("The plan is of the wrong migration mode for this command.")]
    WrongPlanMode,
    /// Note preparation kept producing new rounds past the round bound.
    #[error("Migration did not converge within {0} rounds.")]
    PreparationDidNotConverge(usize),
    #[error(transparent)]
    InvalidParams(#[from] crate::wallet::migration::InvalidMigrationParams),
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// Propose send error.
    #[error("Propose send error.")]
    ProposeSendError(#[from] ProposeSendError),
    /// Propose shield error.
    #[error("Propose shield error.")]
    ProposeShieldError(#[from] ProposeShieldError),
    /// Failed to construct sending transaction.
    #[error("Failed to construct sending transaction. {0}")]
    CalculateSendError(CalculateTransactionError<OutputRef>),
    /// Failed to construct shielding transaction.
    #[error("Failed to construct shielding transaction. {0}")]
    CalculateShieldError(CalculateTransactionError<Infallible>),
    /// Failed to retarget the stored proposal for offline signing.
    #[error("Failed to retarget the stored proposal for offline signing. {0}")]
    RetargetError(zcash_client_backend::proposal::ProposalError),
    /// No proposal found in the wallet.
    #[error("No proposal found in the wallet.")]
    NoStoredProposal,
    /// Transmission error.
    #[error("Transmission error.")]
    TransmissionError(#[from] TransmissionError),
    /// OP_RETURN send error.
    #[error("OP_RETURN send error. {0}")]
    OpReturn(crate::wallet::error::WalletError),
    /// An OP_RETURN proposal cannot be calculated without transmitting.
    /// Its second transaction spends an output of the first.
    #[error("An OP_RETURN proposal cannot be calculated without transmitting.")]
    OpReturnNotCalculable,
    /// The OP_RETURN proposal's source address was reserved by another send
    /// after the proposal was made. Propose again.
    #[error(
        "The OP_RETURN proposal is stale: its source address is no longer next. Propose again."
    )]
    OpReturnSourceAddressStale,
    /// The deshield was transmitted and a later step failed. If
    /// `op_return_txid` is `None`, the proposal is stored again with the
    /// deshield txid and `send_stored_proposal` resumes from the OP_RETURN
    /// step. If it is `Some`, the OP_RETURN transaction is in the wallet
    /// with `Calculated` status and `transmit_calculated` resends it.
    #[error("OP_RETURN send failed after the deshield {deshield_txid} was transmitted. {source}")]
    OpReturnAfterDeshield {
        /// The transmitted deshield.
        deshield_txid: TxId,
        /// The calculated OP_RETURN transaction, if it was built.
        op_return_txid: Option<TxId>,
        /// The failure.
        #[source]
        source: Box<LightClientError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum TransmissionError {
    /// Transmission failed.
    #[error("Transmission failed. {0}")]
    TransmissionFailed(String),
    /// Transaction to transmit does not have `Calculated` status: {0}
    #[error("Transaction to transmit does not have `Calculated` status: {0}")]
    IncorrectTransactionStatus(TxId),
    /// Txid reported by server does not match calculated txid.
    #[error(
        "Server error: txid reported by the server does not match calculated txid.\ncalculated txid:\n{0}\ntxid from server: {1}"
    )]
    IncorrectTxidFromServer(TxId, TxId),
}

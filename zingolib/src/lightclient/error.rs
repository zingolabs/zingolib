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
    /// A mixnet-only surface was attempted while the mixnet was bootstrapping.
    #[cfg(feature = "nym")]
    #[error(transparent)]
    MixnetNotReady(#[from] crate::mixnet::MixnetNotReady),
    /// The mixnet liveness probe was requested while Mixnet Mode is toggled off.
    #[cfg(feature = "nym")]
    #[error(
        "the mixnet liveness probe requires Mixnet Mode, which is off; \
         enable Mixnet Mode to probe Destinations"
    )]
    ProbeRequiresMixnet,
    /// A probe target outside the one endpoint shape the mixnet exit
    /// policy carries.
    #[cfg(feature = "nym")]
    #[error("probe targets must be https on port 443; got '{0}'")]
    IneligibleProbeTarget(http::Uri),
    /// The configured migration transmission target shares the
    /// synchronization endpoint's host, which would let that server correlate
    /// the wallet's sync stream with its migration cohort (ADR 0011,
    /// 2026-07-23).
    #[cfg(feature = "nym")]
    #[error(
        "the migration transmission target '{host}' is the synchronization endpoint; migration \
         parts never go to the sync server. Configure a different migration_transmission_uri or \
         remove it to use the Destination Rotation."
    )]
    MigrationTransmissionTargetIsSyncEndpoint {
        /// The host both endpoints share.
        host: String,
    },
    /// No Destination remains to carry migration parts over the mixnet,
    /// with the typed refusal saying whether exclusion or an empty pool
    /// emptied the draw.
    #[cfg(feature = "nym")]
    #[error(transparent)]
    NoEligibleDestination(#[from] crate::destination::NoEligibleDestinations),
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
    /// ZIP 244 folds the anchor into the signature hash, so a transaction
    /// signed ahead of its boundary commits to the wrong anchor.
    #[error(
        "The pre-signed strategy is unavailable until the Ironwood transaction digest excludes the anchor from the signature hash."
    )]
    PreSignedUnavailable,
    /// The wallet's notes changed between planning and consent, so the
    /// consented plan no longer describes what would be sent.
    #[error("The wallet's notes changed since the plan was displayed. Re-plan and re-confirm.")]
    ConsentStale,
    /// The transmission cadence can change only while every part is unsent.
    #[error("Phase 2 has begun; the transmission cadence can no longer change.")]
    CadenceFixed,
    /// Note splitting kept producing new rounds past the round bound.
    #[error("Migration did not converge within {0} rounds.")]
    SplitDidNotConverge(usize),
    /// A note-splitting transaction failed or disappeared from the wallet.
    #[error("Note-splitting transaction {0} failed or disappeared.")]
    SplitTransactionFailed(TxId),
    /// Note-splitting transactions were not confirmed within the polling
    /// window.
    #[error("Timed out waiting for note-splitting transactions to confirm.")]
    SplitConfirmationTimeout,
    /// The scheduled flow was asked to start over a plan that still needs
    /// note splitting, which no scheduled-flow driver executes yet.
    #[error(
        "The wallet's notes need splitting before a scheduled migration, and the scheduled flow \
         does not drive note splitting yet. Run the immediate migration instead."
    )]
    NoteSplittingRequired,
    /// The one-call immediate path found a consented scheduled migration
    /// and must not collapse its schedule.
    #[error(
        "A consented scheduled migration is in progress. Let it run (auto transmitting), advance \
         it with catch-up, or cancel it before an immediate migration."
    )]
    ScheduledMigrationExists,
    /// The migration in progress belongs to a different account.
    #[error("The migration in progress belongs to a different account.")]
    DifferentAccount,
    /// The Ironwood era is too young to hold a part. A part transmits in one
    /// bucket and anchors in a lower one, and both must sit above the NU6.3
    /// activation, so the earliest window that can hold a part opens two
    /// buckets above the activation's. Until then there is no legal anchor,
    /// whatever the wallet's notes look like.
    #[error(
        "The Ironwood era is too young for a migration part: the first window that can \
         hold one opens at height {retry_after}. Retry after it."
    )]
    IronwoodEraTooYoung {
        /// The first transmission window boundary that can hold a part.
        retry_after: zcash_protocol::consensus::BlockHeight,
    },
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
    /// Swap deposit (OP_RETURN memo carrier) error.
    #[error("Swap deposit error. {0}")]
    SwapDeposit(crate::wallet::error::WalletError),
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

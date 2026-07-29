//! Errors assoicated with [`crate::lightclient::LightClient`].

use std::convert::Infallible;

use zcash_protocol::TxId;

use pepper_sync::error::{SyncError, SyncModeError};
use zingo_netutils::GetClientError;

use crate::wallet::{
    error::{
        CalculateTransactionError, PriceError, ProposeSendError, ProposeShieldError, WalletError,
    },
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
    #[error("Sync error. {0}")]
    SyncError(#[from] SyncError<WalletError>),
    /// Sync mode error.
    #[error("Sync mode error. {0}")]
    SyncModeError(#[from] SyncModeError),
    /// Send error.
    #[error("Send error. {0}")]
    SendError(#[from] SendError),
    /// gPRC client error.
    #[error("gRPC client error. {0}")]
    ClientError(#[from] GetClientError),
    /// Indexer request error.
    #[error("Indexer request error. {0}")]
    IndexerError(#[from] zingo_netutils::Status),
    /// File error.
    #[error("File error. {0}")]
    FileError(std::io::Error),
    /// Wallet error.
    #[error("Wallet error. {0}")]
    WalletError(#[from] WalletError),
    /// Ironwood migration error.
    #[error("Ironwood migration error. {0}")]
    MigrationError(#[from] MigrationError),
    /// No indexer configured. Call set_indexer_uri() to connect before calling network operations.
    #[error("Offline: no indexer configured. Call set_indexer_uri() to connect.")]
    Offline,
    /// Price fetch error.
    #[error("Price fetch error. {0}")]
    PriceError(#[from] PriceError),
    /// The mixnet-routed price fetch was requested while Mixnet Mode is toggled
    /// off. The opt-in route fails closed rather than falling back to clearnet
    /// (ADR 0011); use the clearnet default `update_current_price` instead, or
    /// enable Mixnet Mode (`nym on`).
    #[cfg(feature = "nym")]
    #[error(
        "the mixnet-routed price fetch requires Mixnet Mode, which is off; \
         enable it (`nym on`), or use the clearnet price fetch"
    )]
    PriceFetchRequiresMixnet,
    /// A mixnet-only surface was attempted while the mixnet was bootstrapping.
    #[cfg(feature = "nym")]
    #[error("{0}")]
    MixnetNotReady(#[from] crate::nym::MixnetNotReady),
    /// The configured migration broadcast target shares the synchronization
    /// endpoint's host, which would let that server correlate the wallet's
    /// sync stream with its migration cohort (ADR 0011, 2026-07-23).
    #[cfg(feature = "nym")]
    #[error(
        "the migration broadcast target '{host}' is the synchronization endpoint; migration \
         parts never go to the sync server. Configure a different migration_broadcast_uri or \
         remove it to use the Broadcast Indexer rotation."
    )]
    MigrationBroadcastTargetIsSyncEndpoint {
        /// The host both endpoints share.
        host: String,
    },
    /// Excluding the synchronization endpoint left no Broadcast Indexer to
    /// carry migration parts over the mixnet.
    #[cfg(feature = "nym")]
    #[error(
        "no eligible Broadcast Indexer remains after excluding the synchronization endpoint's \
         host from the curated list"
    )]
    NoEligibleBroadcastIndexer,
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
    /// The broadcast cadence can change only while every part is unsent.
    #[error("Phase 2 has begun; the broadcast cadence can no longer change.")]
    CadenceFixed,
    /// Note splitting kept producing new rounds past the round bound.
    #[error("Migration did not converge within {0} rounds.")]
    SplitDidNotConverge(usize),
    /// A migration transaction (a note split or a part) failed or
    /// disappeared from the wallet.
    #[error("Migration transaction {0} failed or disappeared.")]
    TransactionFailed(TxId),
    /// Migration transactions were not confirmed within the polling window.
    #[error("Timed out waiting for migration transactions to confirm.")]
    ConfirmationTimeout,
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
        "A consented scheduled migration is in progress. Let it run (auto broadcasting), advance \
         it with catch-up, or cancel it before an immediate migration."
    )]
    ScheduledMigrationExists,
    /// The migration in progress belongs to a different account.
    #[error("The migration in progress belongs to a different account.")]
    DifferentAccount,
    /// The Ironwood era is too young to hold a part. A part broadcasts in one
    /// bucket and anchors in a lower one, and both must sit above the NU6.3
    /// activation, so the earliest window that can hold a part opens two
    /// buckets above the activation's. Until then there is no legal anchor,
    /// whatever the wallet's notes look like.
    #[error(
        "The Ironwood era is too young for a migration part: the first window that can \
         hold one opens at height {retry_after}. Retry after it."
    )]
    IronwoodEraTooYoung {
        /// The first broadcast window boundary that can hold a part.
        retry_after: zcash_protocol::consensus::BlockHeight,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// Propose send error.
    #[error("Propose send error. {0}")]
    ProposeSendError(#[from] ProposeSendError),
    /// Propose shield error.
    #[error("Propose shield error. {0}")]
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
    #[error("Transmission error. {0}")]
    TransmissionError(#[from] TransmissionError),
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

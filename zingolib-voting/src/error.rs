//! The voting failure surface for zingolib's LightClient API.
//!
//! One typed error across config resolution, round authentication, journaling,
//! and the round lifecycle. SDK failures travel whole; zingolib-side failures
//! (offline client, pruned snapshot witnesses, a view-only wallet) have their
//! own variants so a consumer can branch on cause without parsing prose.

use zcash_voting::VotingErrorKind;

#[derive(Debug, thiserror::Error)]
pub enum VotingFailure {
    #[error(transparent)]
    Sdk(#[from] zcash_voting::VotingError),
    #[error("the client is offline; connect an indexer before voting")]
    Offline,
    #[error("round {round_id} is not authenticated by the pinned voting config")]
    UnauthenticatedRound { round_id: String },
    #[error("no authenticated voting round is open on the vote servers")]
    NoActiveRound,
    #[error("no voting session is armed for round {round_id}; call begin_voting first")]
    SessionNotArmed { round_id: String },
    #[error("the armed voting session drives round {armed}, not {requested}")]
    RoundMismatch { armed: String, requested: String },
    #[error(
        "the wallet's snapshot checkpoint at height {height} is gone, so note \
         position {position} cannot be witnessed for the round"
    )]
    SnapshotWitnessPruned { height: u64, position: u64 },
    #[error(
        "the witness root at snapshot height {height} does not match the \
         round's authenticated nc_root"
    )]
    WitnessRootMismatch { height: u64 },
    #[error("the wallet holds no spending key; view-only wallets cannot vote")]
    ViewOnlyWallet,
    #[error("the delegation signing request does not match this wallet: {message}")]
    SigningRequestMismatch { message: String },
    #[error("software signing cannot serve this request: {message}")]
    SigningUnsupported { message: String },
    #[error("the vote server answered unusably: {0}")]
    VoteServer(String),
    #[error("voting config resolution failed: {0}")]
    Config(String),
    #[error("voting requires the wallet's spending seed, which is not held in memory")]
    SeedUnavailable,
}

/// The machine-readable kind of a [`VotingFailure`], safe to carry through
/// cloned events and across threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VotingFailureKind {
    Sdk(VotingErrorKind),
    Offline,
    UnauthenticatedRound,
    NoActiveRound,
    SessionNotArmed,
    RoundMismatch,
    SnapshotWitnessPruned,
    WitnessRootMismatch,
    ViewOnlyWallet,
    SigningRequestMismatch,
    SigningUnsupported,
    VoteServer,
    Config,
    SeedUnavailable,
}

impl VotingFailure {
    /// The kind and redacted message of this failure, for typed event payloads.
    pub fn summary(&self) -> VotingFailureSummary {
        let kind = match self {
            VotingFailure::Sdk(error) => VotingFailureKind::Sdk(error.kind()),
            VotingFailure::Offline => VotingFailureKind::Offline,
            VotingFailure::UnauthenticatedRound { .. } => VotingFailureKind::UnauthenticatedRound,
            VotingFailure::NoActiveRound => VotingFailureKind::NoActiveRound,
            VotingFailure::SessionNotArmed { .. } => VotingFailureKind::SessionNotArmed,
            VotingFailure::RoundMismatch { .. } => VotingFailureKind::RoundMismatch,
            VotingFailure::SnapshotWitnessPruned { .. } => VotingFailureKind::SnapshotWitnessPruned,
            VotingFailure::WitnessRootMismatch { .. } => VotingFailureKind::WitnessRootMismatch,
            VotingFailure::ViewOnlyWallet => VotingFailureKind::ViewOnlyWallet,
            VotingFailure::SigningRequestMismatch { .. } => {
                VotingFailureKind::SigningRequestMismatch
            }
            VotingFailure::SigningUnsupported { .. } => VotingFailureKind::SigningUnsupported,
            VotingFailure::VoteServer(_) => VotingFailureKind::VoteServer,
            VotingFailure::Config(_) => VotingFailureKind::Config,
            VotingFailure::SeedUnavailable => VotingFailureKind::SeedUnavailable,
        };
        VotingFailureSummary {
            kind,
            message: self.to_string(),
        }
    }
}

/// A cloneable summary of one [`VotingFailure`]: its typed kind plus the
/// rendered message. Proof material and sighashes never enter the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VotingFailureSummary {
    pub kind: VotingFailureKind,
    pub message: String,
}

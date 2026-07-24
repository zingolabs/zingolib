//! Attribution of send failures to the layer that produced them.
//!
//! A wallet-facing send error names a symptom, not a culprit: the
//! wallet could have built a context-bad transaction, the indexer
//! could have transformed the validator's verdict in transit, or the
//! validator could have wrongly rejected valid bytes. This module
//! generalizes the probe `boundary_rejection_attribution` prototyped:
//! recover the exact rejected bytes from the wallet's Failed record,
//! then judge the SAME bytes through the direct validator channel
//! (bypassing the indexer) at two times — immediately, and again after
//! a few blocks of distance. The two direct verdicts separate the
//! three suspects without trusting the indexer-path error string at
//! all, which matters while zainod 0.6.0 masks validator rejections
//! as opaque Internal errors (zaino#1404).

use zcash_local_net::validator::Validator as _;
use zingolib::lightclient::LightClient;

use crate::setup_metrics::MeteredNet;
use crate::validator_rpc::{self, RawTransactionVerdict};

/// Which layer a send failure is attributed to, judged solely by the
/// validator's direct verdicts on the wallet's retained bytes.
#[derive(Debug)]
pub enum SendFailureAttribution {
    /// The direct submission was ACCEPTED while the indexer path
    /// reported failure: the indexer transformed (or manufactured) the
    /// verdict. Carries the txid the validator returned.
    IndexerTransformed {
        /// The txid of the directly-accepted transaction.
        direct_txid: String,
    },
    /// The identical bytes were rejected at the boundary but accepted
    /// after `distance_blocks`: the proof was valid all along and the
    /// validator's boundary-time verdict was wrong.
    ValidatorWrongAtBoundary {
        /// The validator's rejection message at the boundary.
        boundary_error: String,
        /// The txid returned when the identical bytes were accepted.
        later_txid: String,
    },
    /// The identical bytes were rejected at both times: the wallet
    /// built a transaction that is invalid regardless of chain
    /// context (or valid only under rules the chain never reaches),
    /// and the validator judged correctly both times.
    WalletBuiltInvalid {
        /// The validator's rejection message at the boundary.
        boundary_error: String,
        /// The validator's rejection message after distance.
        later_error: String,
    },
}

impl SendFailureAttribution {
    /// The rejection message the validator gave the ORIGINAL
    /// submission-time judgement, when there was one. This is the
    /// trustworthy replacement for the indexer-path error string when
    /// classifying a rejection.
    pub fn boundary_error(&self) -> Option<&str> {
        match self {
            SendFailureAttribution::IndexerTransformed { .. } => None,
            SendFailureAttribution::ValidatorWrongAtBoundary { boundary_error, .. }
            | SendFailureAttribution::WalletBuiltInvalid { boundary_error, .. } => {
                Some(boundary_error)
            }
        }
    }
}

impl std::fmt::Display for SendFailureAttribution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendFailureAttribution::IndexerTransformed { direct_txid } => write!(
                formatter,
                "INDEXER TRANSFORMED THE VERDICT: the validator accepted the same bytes \
                 directly (txid {direct_txid}) while the indexer path reported failure"
            ),
            SendFailureAttribution::ValidatorWrongAtBoundary {
                boundary_error,
                later_txid,
            } => write!(
                formatter,
                "VALIDATOR VERDICT WAS WRONG AT THE BOUNDARY: identical bytes rejected \
                 ({boundary_error}) then accepted after distance (txid {later_txid})"
            ),
            SendFailureAttribution::WalletBuiltInvalid {
                boundary_error,
                later_error,
            } => write!(
                formatter,
                "WALLET BUILT AN INVALID TRANSACTION: identical bytes rejected at the \
                 boundary ({boundary_error}) and again after distance ({later_error})"
            ),
        }
    }
}

/// Serializes the transaction retained by a Failed record in
/// `client`'s wallet — the exact bytes the validator judged. Wallet
/// transactions live in a map with arbitrary iteration order, so with
/// more than one Failed record the choice among them is arbitrary:
/// probe immediately after the failure under study, while it is the
/// only one.
///
/// # Panics
///
/// Panics when no Failed record exists; call this only after a send
/// has failed.
pub async fn failed_transaction_bytes(client: &LightClient) -> Vec<u8> {
    let wallet = client.wallet();
    let wallet = wallet.read().await;
    let failed_transaction = wallet
        .wallet_transactions
        .values()
        .find(|transaction| transaction.status().is_failed())
        .expect("a failed send must leave a Failed record holding the built transaction");
    let mut bytes = vec![];
    failed_transaction.transaction().write(&mut bytes).unwrap();
    bytes
}

/// Attributes `client`'s send failure to the wallet builder, the
/// indexer transport, or the validator verdict, by
/// judging the retained bytes through the direct validator channel
/// now and again after `distance_blocks` of chain growth.
///
/// The probe mutates chain state: it mines `distance_blocks` blocks
/// whenever the immediate direct submission is rejected, and an
/// [`SendFailureAttribution::IndexerTransformed`] or
/// [`SendFailureAttribution::ValidatorWrongAtBoundary`] outcome leaves
/// the transaction in the validator's mempool. Run it after the
/// experiment's own observations are complete.
///
/// A caveat on the `IndexerTransformed` reading: the direct probe runs
/// after the indexer-path failure, so a verdict that flipped in the
/// gap (a new block arriving between the two submissions) can
/// masquerade as an indexer transformation. Interpret it as "the
/// indexer's report disagrees with the validator's judgement of the
/// same bytes moments later", and corroborate with the observatory
/// records before convicting.
pub async fn attribute_send_failure(
    local_net: &MeteredNet,
    client: &LightClient,
    distance_blocks: u32,
) -> SendFailureAttribution {
    let transaction_bytes = failed_transaction_bytes(client).await;
    let rpc_port = local_net.validator().rpc_listen_port();

    let boundary_error =
        match validator_rpc::send_raw_transaction(rpc_port, &transaction_bytes).await {
            RawTransactionVerdict::Accepted(direct_txid) => {
                return SendFailureAttribution::IndexerTransformed { direct_txid };
            }
            RawTransactionVerdict::Rejected(message) => message,
        };

    local_net
        .validator()
        .generate_blocks(distance_blocks)
        .await
        .unwrap();

    match validator_rpc::send_raw_transaction(rpc_port, &transaction_bytes).await {
        RawTransactionVerdict::Accepted(later_txid) => {
            SendFailureAttribution::ValidatorWrongAtBoundary {
                boundary_error,
                later_txid,
            }
        }
        RawTransactionVerdict::Rejected(later_error) => {
            SendFailureAttribution::WalletBuiltInvalid {
                boundary_error,
                later_error,
            }
        }
    }
}

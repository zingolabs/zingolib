use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;

use pepper_sync::wallet::{
    KeyIdInterface, NoteInterface, OrchardNote, OutgoingNoteInterface, OutputId, OutputInterface,
    SaplingNote, TransparentCoin, WalletTransaction,
};

use super::LightWallet;
use super::error::{FeeError, RemovalError, SpendError};
use super::summary::data::{SendType, TransactionKind};
use crate::config::get_donation_address_for_chain;

impl LightWallet {
    /// Gets all outputs of a given type spent in the given `transaction`.
    pub(super) fn find_spends<Op: OutputInterface>(
        &self,
        transaction: &WalletTransaction,
        fail_on_miss: bool,
    ) -> Result<Vec<&Op>, SpendError> {
        let spends = self
            .wallet_outputs::<Op>()
            .into_iter()
            .filter_map(|output| {
                output.spending_transaction().and_then(|txid| {
                    if txid == transaction.txid() {
                        let spend = Op::transaction_inputs(transaction)
                            .into_iter()
                            .find(|&input| output.spend_link() == Some(input.clone()));

                        if spend.is_none() {
                            return Some(Err(SpendError::IncorrectSpendingTransaction {
                                output_id: output.output_id(),
                                txid,
                            }));
                        }

                        Some(Ok(output))
                    } else {
                        None
                    }
                })
            })
            .collect::<Result<Vec<_>, SpendError>>()?;

        if fail_on_miss {
            let spend_links = spends
                .iter()
                .filter_map(|&spend| spend.spend_link())
                .collect::<Vec<_>>();

            for input in Op::transaction_inputs(transaction) {
                if !spend_links.contains(input) {
                    return Err(SpendError::SpendNotFound {
                        pool: Op::POOL_TYPE,
                        txid: transaction.txid(),
                        spend: format!("{input:?}"),
                    });
                }
            }
        }

        Ok(spends)
    }

    /// Calculate the fee for a transaction in the wallet.
    ///
    /// Fails if transparent spends are not found in the wallet.
    // TODO: write integration test
    pub fn calculate_transaction_fee(
        &self,
        transaction: &WalletTransaction,
    ) -> Result<Zatoshis, FeeError> {
        Ok(transaction
            .transaction()
            .fee_paid(|outpoint| -> Result<Option<Zatoshis>, FeeError> {
                let outpoint = OutputId::from(outpoint);
                let prevout = self
                    .wallet_outputs::<TransparentCoin>()
                    .into_iter()
                    .find(|&output| output.output_id() == outpoint)
                    .ok_or(FeeError::SpendNotFound {
                        txid: transaction.txid(),
                        spend: format!("{outpoint:?}"),
                    })?;

                Ok(Some(
                    Zatoshis::from_u64(prevout.value()).expect("value converted from checked type"),
                ))
            })?
            .expect("fee should not be negative"))
    }

    /// Removes transaction with the given `txid` from the wallet.
    /// Also sets the `spending_transaction` fields of any outputs spent in this transaction to `None` restoring the
    /// wallet balance and allowing these outputs to be re-selected for spending in future sends.
    ///
    /// # Error
    ///
    /// Returns error if transaction is confirmed or does not exist in the wallet.
    pub fn remove_unconfirmed_transaction(&mut self, txid: TxId) -> Result<(), RemovalError> {
        if let Some(transaction) = self.wallet_transactions.get(&txid) {
            if transaction.status().is_confirmed() {
                return Err(RemovalError::TransactionAlreadyConfirmed);
            }
        } else {
            return Err(RemovalError::TransactionNotFound);
        }

        pepper_sync::reset_spends(&mut self.wallet_transactions, vec![txid]);
        self.wallet_transactions.remove(&txid);
        self.save_required = true;

        Ok(())
    }

    /// Determine the kind of transaction from the current state of wallet data.
    pub(crate) fn transaction_kind(
        &self,
        transaction: &WalletTransaction,
    ) -> Result<TransactionKind, SpendError> {
        let zfz_address = get_donation_address_for_chain(&self.network);

        let transparent_spends = self.find_spends::<TransparentCoin>(transaction, false)?;
        let sapling_spends = self.find_spends::<SaplingNote>(transaction, false)?;
        let orchard_spends = self.find_spends::<OrchardNote>(transaction, false)?;

        if transparent_spends.is_empty()
            && sapling_spends.is_empty()
            && orchard_spends.is_empty()
            && transaction.outgoing_sapling_notes().is_empty()
            && transaction.outgoing_orchard_notes().is_empty()
        {
            Ok(TransactionKind::Received)
        } else if !transparent_spends.is_empty()
            && sapling_spends.is_empty()
            && orchard_spends.is_empty()
            && transaction.outgoing_sapling_notes().is_empty()
            && transaction.outgoing_orchard_notes().is_empty()
            && (!transaction.orchard_notes().is_empty() || !transaction.sapling_notes().is_empty())
        {
            Ok(TransactionKind::Sent(SendType::Shield))
        } else if transaction
            .transaction()
            .transparent_bundle()
            .is_none_or(|bundle| bundle.vout.len() == transaction.transparent_coins().len())
            && transaction
                .outgoing_sapling_notes()
                .iter()
                .all(|outgoing_note| {
                    self.is_sapling_address_in_unified_addresses(&outgoing_note.note().recipient())
                        .is_some()
                        || outgoing_note.key_id().scope == zip32::Scope::Internal
                        || outgoing_note
                            .encoded_recipient_full_unified_address(&self.network)
                            .is_some_and(|unified_address| unified_address == *zfz_address)
                })
            && transaction
                .outgoing_orchard_notes()
                .iter()
                .all(|outgoing_note| {
                    self.is_orchard_address_in_unified_addresses(&outgoing_note.note().recipient())
                        .is_some()
                        || outgoing_note.key_id().scope == zip32::Scope::Internal
                        || outgoing_note
                            .encoded_recipient_full_unified_address(&self.network)
                            .is_some_and(|unified_address| unified_address == *zfz_address)
                })
        {
            Ok(TransactionKind::Sent(SendType::SendToSelf))
        } else {
            Ok(TransactionKind::Sent(SendType::Send))
        }
    }
}

/// Returns all unspent notes of the specified pool and `account` in the given `transaction`.
///
/// Any output IDs in `exclude` will not be returned.
/// `spend_horizon` is the block height from which all nullifiers have been mapped, guaranteeing a note from a block
/// equal to or above this height is unspent.
/// If `include_potentially_spent_notes` is `true`, notes will be included even if the wallet's current sync state
/// cannot guarantee the notes are unspent. In this case, the `spend_horizon` is not used.
pub(crate) fn transaction_unspent_notes<'a, N: NoteInterface + 'a>(
    transaction: &'a WalletTransaction,
    exclude: &'a [OutputId],
    account: zip32::AccountId,
    spend_horizon: BlockHeight,
    include_potentially_unspent_notes: bool,
) -> impl Iterator<Item = &'a N> + 'a {
    let guaranteed_unspent = if include_potentially_unspent_notes {
        true
    } else {
        transaction.status().get_height() >= spend_horizon
    };

    N::transaction_outputs(transaction)
        .iter()
        .filter(move |&note| {
            note.spending_transaction().is_none()
                && !exclude.contains(&note.output_id())
                && note.key_id().account_id() == account
                && guaranteed_unspent
        })
}

/// Returns all unspent transparent outputs in the given `transaction`.
pub(crate) fn transaction_unspent_coins(
    transaction: &WalletTransaction,
) -> impl Iterator<Item = &TransparentCoin> {
    TransparentCoin::transaction_outputs(transaction)
        .iter()
        .filter(move |&coin| coin.spending_transaction().is_none())
}

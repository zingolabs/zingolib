use zcash_primitives::transaction::TxId;
use zcash_protocol::value::ZatBalance;

use super::{
    LightWallet,
    error::{FeeError, RemovalError, SpendError},
};
use pepper_sync::wallet::{OutputId, OutputInterface, TransparentCoin, WalletTransaction};

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
                            .find(|&input| (output.spend_link() == Some(input.clone())));

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
                .flat_map(|&spend| spend.spend_link())
                .collect::<Vec<_>>();

            for input in Op::transaction_inputs(transaction) {
                if !spend_links.contains(input) {
                    return Err(SpendError::SpendNotFound {
                        pool: Op::POOL_TYPE,
                        txid: transaction.txid(),
                        spend: format!("{:?}", input),
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
    ) -> Result<u64, FeeError> {
        Ok(transaction
            .transaction()
            .fee_paid(|outpoint| -> Result<ZatBalance, FeeError> {
                let outpoint = OutputId::from(outpoint);
                let prevout = self
                    .wallet_outputs::<TransparentCoin>()
                    .into_iter()
                    .find(|&output| output.output_id() == outpoint)
                    .ok_or(FeeError::SpendNotFound {
                        txid: transaction.txid(),
                        spend: format!("{:?}", outpoint),
                    })?;

                Ok(ZatBalance::from_u64(prevout.value())
                    .expect("value converted from checked type"))
            })?
            .try_into()
            .expect("fee should not be negative"))
    }

    /// Removes transaction with the given `txid` from the wallet.
    /// Also sets the `spending_transaction` fields of any outputs spent in this transaction to `None` allowing these
    /// outputs to be re-selected for spending in future sends.
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
        };

        // TODO: could be added as an API to pepper-sync
        self.wallet_transactions
            .values_mut()
            .flat_map(|transaction| transaction.transparent_coins_mut())
            .filter(|output| (output.spending_transaction() == Some(txid)))
            .for_each(|output| {
                output.set_spending_transaction(None);
            });
        self.wallet_transactions
            .values_mut()
            .flat_map(|transaction| transaction.sapling_notes_mut())
            .filter(|output| (output.spending_transaction() == Some(txid)))
            .for_each(|output| {
                output.set_spending_transaction(None);
            });
        self.wallet_transactions
            .values_mut()
            .flat_map(|transaction| transaction.orchard_notes_mut())
            .filter(|output| (output.spending_transaction() == Some(txid)))
            .for_each(|output| {
                output.set_spending_transaction(None);
            });
        self.wallet_transactions
            .remove(&txid)
            .expect("transaction checked to exist");
        self.save_required = true;

        Ok(())
    }
}

/// Returns all unspent outputs of the specified pool in the given `transaction`.
///
/// Any output IDs in `exclude` will not be returned.
pub(crate) fn transaction_unspent_outputs<'a, Op: OutputInterface + 'a>(
    transaction: &'a WalletTransaction,
    exclude: &'a [OutputId],
) -> impl Iterator<Item = &'a Op> + 'a {
    Op::transaction_outputs(transaction)
        .iter()
        .filter(|&output| {
            output.spending_transaction().is_none() && !exclude.contains(&output.output_id())
        })
}

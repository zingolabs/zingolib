use zcash_client_backend::{wallet::NoteId, ShieldedProtocol};
use zcash_primitives::transaction::components::Amount;
use zingo_sync::primitives::{OutputId, OutputInterface, TransparentCoin, WalletTransaction};

use crate::ShieldedDomain;

use super::{
    error::{FeeError, KindError},
    LightWallet,
};

impl LightWallet {
    /// Gets all outputs of a given type spent in the given `transaction`.
    pub(super) fn find_spends<Op: OutputInterface>(
        &self,
        transaction: &WalletTransaction,
        fail_on_miss: bool,
    ) -> Result<Vec<&Op>, KindError> {
        let spends = self.wallet_outputs::<Op>()
            .into_iter()
            .filter_map(|output| {
                output.spending_transaction().and_then(|txid| {
                    if txid == transaction.txid() {
                        let spend = Op::transaction_inputs(transaction)
                            .into_iter()
                            .find(|&input| {
                                output.spend_link().map_or(false, |spend_link|
                                {
                                    *input
                                    == spend_link
                                })
                            });

                        if spend.is_none() {
                            // TODO: error handling
                            panic!("output's spending transaction field incorrectly points to transaction which did not spend output!");
                        }

                        Some(output)
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();

        if fail_on_miss {
            let spend_links = spends
                .iter()
                .flat_map(|&spend| spend.spend_link())
                .collect::<Vec<_>>();

            for input in Op::transaction_inputs(transaction) {
                if !spend_links.contains(input) {
                    return Err(KindError::SpendNotFound {
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
    pub fn calculate_transaction_fee(
        &self,
        transaction: &WalletTransaction,
    ) -> Result<u64, FeeError> {
        Ok(transaction
            .transaction()
            .fee_paid(|outpoint| -> Result<Amount, FeeError> {
                let outpoint = OutputId::from(outpoint);
                let prevout = self
                    .wallet_outputs::<TransparentCoin>()
                    .into_iter()
                    .find(|&output| output.output_id == outpoint)
                    .ok_or(FeeError::SpendNotFound {
                        txid: transaction.txid(),
                        spend: format!("{:?}", outpoint),
                    })?;

                Ok(Amount::from_u64(prevout.value()).expect("value converted from checked type"))
            })?
            .try_into()
            .expect("fee should not be negative"))
    }

    // TODO: this is a method on the wallet so we can implement checking the witness can be constructed also
    pub(crate) fn transaction_spendable_notes<D: ShieldedDomain>(
        &self,
        sources: &[ShieldedProtocol],
        exclude: &[NoteId],
        transaction: &WalletTransaction,
    ) {
        if sources.contains(&D::SHIELDED_PROTOCOL) {
            D::Output::transaction_outputs(transaction)
                .iter()
                .filter_map(|note| {
                    if note.spending_transaction().is_none() {
                        let id = NoteId::new(
                            transaction.txid(),
                            D::SHIELDED_PROTOCOL,
                            note.output_id().output_index() as u16,
                        );
                        if !exclude.contains(&id) {
                            Some((id, note.value()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
            // TODO: return iterator or empty if else
        }
    }
}

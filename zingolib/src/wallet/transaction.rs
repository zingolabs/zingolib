use zcash_primitives::transaction::components::Amount;
use zingo_sync::primitives::{OutputId, OutputInterface, TransparentCoin, WalletTransaction};

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
}

//! This mod contains pieces of the impl LightWallet that are invoked during a send.

use log::error;
use zcash_address::AddressKind;
use zcash_client_backend::proposal::Proposal;
use zcash_proofs::prover::LocalTxProver;

use std::cmp;
use std::ops::DerefMut as _;

use zcash_client_backend::zip321::TransactionRequest;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::memo::Memo;
use zcash_primitives::memo::MemoBytes;

use zingo_memo::create_wallet_internal_memo_version_1;

use super::LightWallet;

/// TODO: Add Doc Comment Here!
#[derive(Debug, Clone)]
pub struct SendProgress {
    /// TODO: Add Doc Comment Here!
    pub id: u32,
    /// TODO: Add Doc Comment Here!
    pub is_send_in_progress: bool,
    /// TODO: Add Doc Comment Here!
    pub progress: u32,
    /// TODO: Add Doc Comment Here!
    pub total: u32,
    /// TODO: Add Doc Comment Here!
    pub last_result: Option<Result<serde_json::Value, String>>,
}

impl SendProgress {
    /// TODO: Add Doc Comment Here!
    pub fn new(id: u32) -> Self {
        SendProgress {
            id,
            is_send_in_progress: false,
            progress: 0,
            total: 0,
            last_result: None,
        }
    }
}

impl LightWallet {
    /// Determines the target height for a transaction, and the offset from which to
    /// select anchors, based on the current synchronised block chain.
    pub(crate) async fn get_target_height_and_anchor_offset(&self) -> Option<(u32, usize)> {
        let range = {
            let blocks = self.last_100_blocks.read().await;
            (
                blocks.last().map(|block| block.height as u32),
                blocks.first().map(|block| block.height as u32),
            )
        };
        match range {
            (Some(min_height), Some(max_height)) => {
                let target_height = max_height + 1;

                // Select an anchor ANCHOR_OFFSET back from the target block,
                // unless that would be before the earliest block we have.
                let anchor_height = cmp::max(
                    target_height
                        .saturating_sub(self.transaction_context.config.reorg_buffer_offset),
                    min_height,
                );

                Some((target_height, (target_height - anchor_height) as usize))
            }
            _ => None,
        }
    }

    // Reset the send progress status to blank
    pub(crate) async fn reset_send_progress(&self) {
        let mut g = self.send_progress.write().await;
        let next_id = g.id + 1;

        // Discard the old value, since we are replacing it
        let _ = std::mem::replace(&mut *g, SendProgress::new(next_id));
    }

    /// Get the current sending status.
    pub async fn get_send_progress(&self) -> SendProgress {
        self.send_progress.read().await.clone()
    }
}

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum BuildTransactionError {
    #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
    NoSpendCapability,
    #[error("Could not load sapling_params: {0:?}")]
    SaplingParams(String),
    #[error("Could not find UnifiedSpendKey: {0:?}")]
    UnifiedSpendKey(#[from] crate::wallet::error::KeyError),
    #[error("Can't Calculate {0:?}")]
    Calculation(
        #[from]
        zcash_client_backend::data_api::error::Error<
            crate::wallet::tx_map::TxMapTraitError,
            std::convert::Infallible,
            std::convert::Infallible,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
    #[error("Only tex multistep transactions are supported!")]
    NonTexMultiStep,
}

impl LightWallet {
    pub(crate) async fn create_transaction<NoteRef>(
        &self,
        proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
    ) -> Result<(), BuildTransactionError> {
        if self
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .is_none()
        {
            return Err(BuildTransactionError::NoSpendCapability);
        }

        // Reset the progress to start. Any errors will get recorded here
        self.reset_send_progress().await;

        let (sapling_output, sapling_spend): (Vec<u8>, Vec<u8>) =
            crate::wallet::utils::read_sapling_params()
                .map_err(BuildTransactionError::SaplingParams)?;
        let sapling_prover =
            zcash_proofs::prover::LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

        match proposal.steps().len() {
            1 => {
                self.create_transaction_helper(sapling_prover, proposal)
                    .await
            }
            2 if proposal.steps()[1]
                .transaction_request()
                .payments()
                .values()
                .any(|payment| {
                    matches!(payment.recipient_address().kind(), AddressKind::Tex(_))
                }) =>
            {
                self.create_transaction_helper(sapling_prover, proposal)
                    .await
            }

            _ => Err(BuildTransactionError::NonTexMultiStep),
        }
    }

    async fn create_transaction_helper<NoteRef>(
        &self,
        sapling_prover: LocalTxProver,
        proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
    ) -> Result<(), BuildTransactionError> {
        let mut wallet_db = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        let usk = &self
            .transaction_context
            .key
            .unified_key_store()
            .try_into()?;
        zcash_client_backend::data_api::wallet::create_proposed_transactions(
            wallet_db.deref_mut(),
            &self.transaction_context.config.chain,
            &sapling_prover,
            &sapling_prover,
            usk,
            zcash_client_backend::wallet::OvkPolicy::Sender,
            proposal,
            Some(self.wallet_capability().first_sapling_address()),
        )?;
        Ok(())
    }
}

// TODO: move to a more suitable place
pub(crate) fn change_memo_from_transaction_request(
    request: &TransactionRequest,
    mut number_of_rejection_addresses: u32,
) -> MemoBytes {
    let mut recipient_uas = Vec::new();
    let mut rejection_address_indexes = Vec::new();
    for payment in request.payments().values() {
        match payment.recipient_address().kind() {
            AddressKind::Unified(ua) => {
                if let Ok(ua) = UnifiedAddress::try_from(ua.clone()) {
                    recipient_uas.push(ua);
                }
            }
            AddressKind::Tex(_) => {
                rejection_address_indexes.push(number_of_rejection_addresses);

                number_of_rejection_addresses += 1;
            }
            _ => (),
        }
    }
    let uas_bytes = match create_wallet_internal_memo_version_1(
        recipient_uas.as_slice(),
        rejection_address_indexes.as_slice(),
    ) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::error!(
                "Could not write uas to memo field: {e}\n\
        Your wallet will display an incorrect sent-to address. This is a visual error only.\n\
        The correct address was sent to."
            );
            [0; 511]
        }
    };
    MemoBytes::from(Memo::Arbitrary(Box::new(uas_bytes)))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use zcash_address::ZcashAddress;
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_primitives::{
        memo::{Memo, MemoBytes},
        transaction::components::amount::NonNegativeAmount,
    };

    use crate::data::receivers::{transaction_request_from_receivers, Receivers};

    #[test]
    fn test_build_request() {
        let amount_1 = NonNegativeAmount::const_from_u64(20000);
        let recipient_address_1 =
            ZcashAddress::try_from_encoded("utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05").unwrap();
        let memo_1 = None;

        let amount_2 = NonNegativeAmount::const_from_u64(20000);
        let recipient_address_2 =
            ZcashAddress::try_from_encoded("utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05").unwrap();
        let memo_2 = Some(MemoBytes::from(
            Memo::from_str("the lake wavers along the beach").expect("string can memofy"),
        ));

        let rec: Receivers = vec![
            crate::data::receivers::Receiver {
                recipient_address: recipient_address_1,
                amount: amount_1,
                memo: memo_1,
            },
            crate::data::receivers::Receiver {
                recipient_address: recipient_address_2,
                amount: amount_2,
                memo: memo_2,
            },
        ];
        let request: TransactionRequest =
            transaction_request_from_receivers(rec).expect("rec can requestify");

        assert_eq!(
            request.total().expect("total"),
            (amount_1 + amount_2).expect("add")
        );
    }
}

//! This mod contains pieces of the impl LightWallet that are invoked during a send.
use crate::wallet::keys::keystore::Keystore;
use crate::wallet::now;

use futures::Future;

use hdwallet::traits::Deserialize as _;
use log::error;
use zcash_client_backend::proposal::Proposal;
use zcash_keys::keys::UnifiedSpendingKey;
use zcash_primitives::transaction::builder::BuildResult;

use std::cmp;
use std::ops::DerefMut as _;

use zcash_client_backend::zip321::TransactionRequest;
use zcash_keys::address::Address;
use zcash_primitives::transaction::Transaction;
use zcash_primitives::{consensus::BlockHeight, memo::Memo};
use zcash_primitives::{memo::MemoBytes, transaction::TxId};

use zingo_memo::create_wallet_internal_memo_version_0;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::LightWallet;

use super::utils::get_price;

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
    pub last_result: Option<Result<String, String>>,
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
    pub(super) async fn get_target_height_and_anchor_offset(&self) -> Option<(u32, usize)> {
        let range = {
            let blocks = self.blocks.read().await;
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

use thiserror::Error;
#[allow(missing_docs)] // error types document themselves
#[derive(Debug, Error)]
pub enum BuildTransactionError {
    #[error("No witness trees. This is viewkey watch, not spendkey wallet.")]
    NoSpendCapability,
    #[error("Could not load sapling_params: {0:?}")]
    SaplingParams(String),
    #[error("Could not find UnifiedSpendKey: {0:?}")]
    UnifiedSpendKey(std::io::Error),
    #[error("Can't Calculate {0:?}")]
    Calculation(
        #[from]
        zcash_client_backend::data_api::error::Error<
            crate::wallet::tx_map_and_maybe_trees::TxMapAndMaybeTreesTraitError,
            std::convert::Infallible,
            std::convert::Infallible,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
    #[error("Sending to exchange addresses is not supported yet!")]
    ExchangeAddressesNotSupported,
}

impl LightWallet {
    pub(crate) async fn build_transaction<NoteRef>(
        &self,
        proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
    ) -> Result<BuildResult, BuildTransactionError> {
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
        
        let Keystore::InMemory(ref wc) = *self.keystore() else {
            unreachable!("Known to be InMemory due to new_from_phrase impl")
        };
        
        let (sapling_output, sapling_spend): (Vec<u8>, Vec<u8>) =
            crate::wallet::utils::read_sapling_params()
                .map_err(BuildTransactionError::SaplingParams)?;
        let sapling_prover =
            zcash_proofs::prover::LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
        let unified_spend_key = UnifiedSpendingKey::try_from(wc)
            .map_err(BuildTransactionError::UnifiedSpendKey)?;

        // We don't support zip320 yet. Only one step.
        if proposal.steps().len() != 1 {
            return Err(BuildTransactionError::ExchangeAddressesNotSupported);
        }

        let step = proposal.steps().first();

        // The 'UnifiedSpendingKey' we create is not a 'proper' USK, in that the
        // transparent key it contains is not the account spending key, but the
        // externally-scoped derivative key. The goal is to fix this, but in the
        // interim we use this special-case logic.
        fn usk_to_tkey(
            unified_spend_key: &UnifiedSpendingKey,
            t_metadata: &zcash_client_backend::wallet::TransparentAddressMetadata,
        ) -> secp256k1::SecretKey {
            hdwallet::ExtendedPrivKey::deserialize(&unified_spend_key.transparent().to_bytes())
                .expect("This a hack to do a type conversion, and will not fail")
                .derive_private_key(t_metadata.address_index().into())
                // This is unwrapped in librustzcash, so I'm not too worried about it
                .expect("private key derivation failed")
                .private_key
        }

        Ok(
            zcash_client_backend::data_api::wallet::calculate_proposed_transaction(
                self.transaction_context
                    .transaction_metadata_set
                    .write()
                    .await
                    .deref_mut(),
                &self.transaction_context.config.chain,
                &sapling_prover,
                &sapling_prover,
                &unified_spend_key,
                zcash_client_backend::wallet::OvkPolicy::Sender,
                proposal.fee_rule(),
                proposal.min_target_height(),
                &[],
                step,
                Some(usk_to_tkey),
                Some(self.keystore().first_sapling_address()),
            )?,
        )
    }

    pub(crate) async fn send_to_addresses_inner<F, Fut>(
        &self,
        transaction: &Transaction,
        submission_height: BlockHeight,
        broadcast_fn: F,
    ) -> Result<TxId, String>
    where
        F: Fn(Box<[u8]>) -> Fut,
        Fut: Future<Output = Result<String, String>>,
    {
        {
            self.send_progress.write().await.is_send_in_progress = false;
        }

        // Create the transaction bytes
        let mut raw_transaction = vec![];
        transaction.write(&mut raw_transaction).unwrap();

        let serverz_transaction_id =
            broadcast_fn(raw_transaction.clone().into_boxed_slice()).await?;

        {
            let price = self.price.read().await.clone();

            let status = ConfirmationStatus::Transmitted(submission_height);
            self.transaction_context
                .scan_full_tx(
                    transaction,
                    status,
                    Some(now() as u32),
                    get_price(now(), &price),
                )
                .await;
        }

        let calculated_txid = transaction.txid();

        let accepted_txid = match crate::utils::conversion::txid_from_hex_encoded_str(
            serverz_transaction_id.as_str(),
        ) {
            Ok(serverz_txid) => {
                if calculated_txid != serverz_txid {
                    // happens during darkside tests
                    error!(
                        "served txid {} does not match calulated txid {}",
                        serverz_txid, calculated_txid,
                    );
                };
                if self.transaction_context.config.accept_server_txids {
                    serverz_txid
                } else {
                    calculated_txid
                }
            }
            Err(e) => {
                error!("server returned invalid txid {}", e);
                calculated_txid
            }
        };

        Ok(accepted_txid)
    }
}

// TODO: move to a more suitable place
pub(crate) fn change_memo_from_transaction_request(request: &TransactionRequest) -> MemoBytes {
    let recipient_uas = request
        .payments()
        .iter()
        .filter_map(|(_, payment)| match payment.recipient_address {
            Address::Transparent(_) => None,
            Address::Sapling(_) => None,
            Address::Unified(ref ua) => Some(ua.clone()),
        })
        .collect::<Vec<_>>();
    let uas_bytes = match create_wallet_internal_memo_version_0(recipient_uas.as_slice()) {
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

    use crate::config::ChainType;
    use zcash_client_backend::{address::Address, zip321::TransactionRequest};
    use zcash_primitives::{
        memo::{Memo, MemoBytes},
        transaction::components::amount::NonNegativeAmount,
    };

    use crate::data::receivers::{transaction_request_from_receivers, Receivers};

    #[test]
    fn test_build_request() {
        let amount_1 = NonNegativeAmount::const_from_u64(20000);
        let recipient_address_1 =
            Address::decode(&ChainType::Testnet, "utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05").unwrap();
        let memo_1 = None;

        let amount_2 = NonNegativeAmount::const_from_u64(20000);
        let recipient_address_2 =
            Address::decode(&ChainType::Testnet, "utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05").unwrap();
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

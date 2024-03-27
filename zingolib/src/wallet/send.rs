use crate::error::{ZingoLibError, ZingoLibResult};

use crate::wallet::now;

use futures::Future;

use sapling_crypto::prover::{OutputProver, SpendProver};

use tokio::sync::RwLockWriteGuard;
use zcash_client_backend::data_api::WalletRead;

use zcash_client_backend::zip321::{Payment, TransactionRequest, Zip321Error};

use zcash_primitives::consensus::Parameters;
use zcash_primitives::zip32::AccountId;

use std::ops::DerefMut;

use zcash_client_backend::address;

use zcash_keys::keys::UnifiedSpendingKey;
use zcash_primitives::memo::MemoBytes;

use zcash_primitives::transaction::components::amount::NonNegativeAmount;

use zcash_primitives::transaction::TxId;
use zcash_primitives::{consensus::BlockHeight, transaction::builder::Builder};

use zingo_status::confirmation_status::ConfirmationStatus;

use self::errors::SendToAddressesError;

use super::record_book::RefRecordBook;

use super::transactions::{Proposa, TxMapAndMaybeTrees};
use super::utils::get_price;
use super::Pool;
use crate::wallet::spend_kit::SpendKit;

#[derive(Debug, Clone)]
pub struct SendProgress {
    pub id: u32,
    pub is_send_in_progress: bool,
    pub progress: u32,
    pub total: u32,
    pub last_error: Option<String>,
    pub last_transaction_id: Option<String>,
}

pub(crate) type NoteSelectionPolicy = Vec<Pool>;

impl SendProgress {
    pub fn new(id: u32) -> Self {
        SendProgress {
            id,
            is_send_in_progress: false,
            progress: 0,
            total: 0,
            last_error: None,
            last_transaction_id: None,
        }
    }
}

type Receivers = Vec<(address::Address, NonNegativeAmount, Option<MemoBytes>)>;

pub fn build_transaction_request_from_tuples<Params: Parameters>(
    params: Params,
    destination_amount_opt_memo_s: Vec<(&str, u64, Option<MemoBytes>)>,
) -> Result<TransactionRequest, Zip321Error> {
    let mut payments = vec![];
    for dam in destination_amount_opt_memo_s {
        let address = zcash_client_backend::address::Address::decode(&params, dam.0).ok_or(
            Zip321Error::ParseError(format!("Invalid recipient address: '{}'", dam.0)),
        )?;

        let value = NonNegativeAmount::from_u64(dam.1).unwrap();

        payments.push(Payment {
            recipient_address: address,
            amount: value,
            memo: dam.2,
            label: None,
            message: None,
            other_params: vec![],
        });
    }

    TransactionRequest::new(payments)
}

type TxBuilder<'a> = Builder<'a, zingoconfig::ChainType, ()>;
impl super::LightWallet {
    // Reset the send progress status to blank
    async fn reset_send_progress(&self) {
        let mut g = self.send_progress.write().await;
        let next_id = g.id + 1;

        // Discard the old value, since we are replacing it
        let _ = std::mem::replace(&mut *g, SendProgress::new(next_id));
    }

    // Get the current sending status.
    pub async fn get_send_progress(&self) -> SendProgress {
        self.send_progress.read().await.clone()
    }

    /// Propose a transfer, calculate fees, and hold the proposal in memory. (1, except in the z->tt case where there will be 2.). If a previous transfer was proposed but not sent, it will be overwritten in the buffer.
    pub async fn propose_transfer(&self, request: TransactionRequest) -> ZingoLibResult<Proposa> {
        let mut context_write_lock: RwLockWriteGuard<'_, TxMapAndMaybeTrees> = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        let mut spend_kit = self.assemble_spend_kit(&mut context_write_lock).await?;
        let proposal = spend_kit.create_proposal(request)?;
        Ok(proposal)
    }

    /// Create and broadcast the most recently proposed transfer.
    pub async fn send_proposed_transfer<F, Fut, P>(
        &self,
        submission_height: BlockHeight,
        broadcast_fn: F,
        sapling_prover: P,
    ) -> Result<Vec<TxId>, SendToAddressesError>
    where
        F: Fn(Box<[u8]>) -> Fut + Clone,
        Fut: Future<Output = Result<String, String>>,
        P: SpendProver + OutputProver,
    {
        // unlock the records to create the proposed transfer.
        let mut context_write_lock: RwLockWriteGuard<'_, TxMapAndMaybeTrees> = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        let proposal = if let Some(ref spending_data) = context_write_lock.spending_data {
            spending_data.latest_proposal.clone()
        } else {
            None // replace with zingolib error return
                 // return ZingoLibError::ViewkeyCantSpend
        };
        let mut spend_kit = self
            .assemble_spend_kit(&mut context_write_lock)
            .await
            .unwrap(); // replace unwrap with zingolib error
        let sending_txids = if let Some(proposal) = proposal {
            spend_kit
                .create_transactions(sapling_prover, proposal)
                .unwrap()
            // replace unwrap with zingolib error
        } else {
            return Err(SendToAddressesError::NoProposal);
        };

        // review! what does this do? is it necessary? why does it seem wrong?
        {
            self.send_progress.write().await.is_send_in_progress = false;
        }

        // send each transaction in the proposition.
        let mut sent_txids = vec![];
        for txid in sending_txids {
            let transaction = spend_kit.get_transaction(txid).unwrap();
            let mut raw_tx = vec![];
            transaction.write(&mut raw_tx).unwrap();
            // .map_err(|e| ZingoLibError::CalculatedTransactionEncode(e.to_string()))?;

            // broadcast the raw transaction to the lightserver
            match broadcast_fn(raw_tx.clone().into_boxed_slice()).await {
                Ok(_) => sent_txids.push(txid),
                Err(e) => {
                    return Err(if sent_txids.is_empty() {
                        SendToAddressesError::NoBroadcast(e)
                    } else {
                        SendToAddressesError::PartialBroadcast(sent_txids, e)
                    })
                }
            };

            let price = self.price.read().await.clone();
            let status = ConfirmationStatus::Broadcast(submission_height);
            // and add the sent transaction as a pending (Broadcast) record.
            self.transaction_context
                .scan_full_tx(&transaction, status, now() as u32, get_price(now(), &price))
                .await;
        }
        if sent_txids.is_empty() {
            Err(SendToAddressesError::NoProposal)
        } else {
            Ok(sent_txids)
        }
    }

    pub async fn assemble_spend_kit<'lock, 'reflock, 'trees, 'book>(
        &'lock self,
        context_write_lock: &'reflock mut RwLockWriteGuard<'lock, TxMapAndMaybeTrees>,
    ) -> ZingoLibResult<SpendKit<'book, 'trees>>
    where
        'lock: 'trees + 'book,
        'reflock: 'trees + 'book,
    {
        if let TxMapAndMaybeTrees {
            spending_data: Some(spending_data),
            current: all_remote_transactions,
        } = context_write_lock.deref_mut()
        {
            Ok(SpendKit::<'book, 'trees> {
                key: {
                    let (mnemonic, _) = self.mnemonic().expect("should have spend capability");
                    let seed = mnemonic.to_seed("");
                    let account_id = AccountId::ZERO;
                    UnifiedSpendingKey::from_seed(
                        &self.transaction_context.config.chain,
                        &seed,
                        account_id,
                    )
                    .expect("should be able to create a unified spend key")
                },
                params: self.transaction_context.config.chain,
                record_book: RefRecordBook::new_from_remote_txid_hashmap(all_remote_transactions), //review! if there are already pending transactions, dont assemble a spend_kit
                trees: &mut spending_data.witness_trees,
                latest_proposal: &mut spending_data.latest_proposal,
                local_sending_transactions: Vec::new(),
            })
        } else {
            Err(ZingoLibError::ViewkeyCantSpend)
        }
    }
}

pub mod errors;

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use zcash_client_backend::{address::Address, zip321::TransactionRequest};
    use zcash_primitives::{
        memo::{Memo, MemoBytes},
        transaction::components::amount::NonNegativeAmount,
    };
    use zingoconfig::ChainType;

    use super::{build_transaction_request_from_tuples, Receivers};

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
            (recipient_address_1, amount_1, memo_1),
            (recipient_address_2, amount_2, memo_2),
        ];
        let request: TransactionRequest =
            build_transaction_request_from_tuples(rec).expect("rec can requestify");

        assert_eq!(
            request.total().expect("total"),
            (amount_1 + amount_2).expect("add")
        );
    }
}

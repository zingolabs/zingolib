use crate::error::{ZingoLibError, ZingoLibResult};
use crate::wallet::data::SpendableSaplingNote;
use crate::wallet::notes::NoteInterface;
use crate::wallet::now;

use futures::Future;

use log::{error, info};

use orchard::note_encryption::OrchardDomain;

use rand::rngs::OsRng;

use sapling_crypto::note_encryption::SaplingDomain;
use sapling_crypto::prover::{OutputProver, SpendProver};

use shardtree::error::{QueryError, ShardTreeError};
use tokio::sync::RwLockWriteGuard;
use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::zip321::{Payment, TransactionRequest, Zip321Error};
use zcash_primitives::transaction::fees::zip317::FeeRule;
use zcash_primitives::zip32::AccountId;

use std::convert::Infallible;
use std::ops::DerefMut;
use std::sync::mpsc::channel;

use zcash_client_backend::{address, proposal};

use zcash_keys::keys::UnifiedSpendingKey;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::transaction::builder::{BuildResult, Progress};
use zcash_primitives::transaction::components::amount::NonNegativeAmount;
use zcash_primitives::transaction::fees::fixed::FeeRule as FixedFeeRule;
use zcash_primitives::transaction::{self, Transaction, TxId};
use zcash_primitives::{
    consensus::BlockHeight,
    legacy::Script,
    memo::Memo,
    transaction::{
        builder::Builder,
        components::{Amount, OutPoint, TxOut},
        fees::zip317::MINIMUM_FEE,
    },
};
use zingo_memo::create_wallet_internal_memo_version_0;
use zingo_status::confirmation_status::ConfirmationStatus;

use self::errors::SendToAddressesError;

use super::data::{SpendableOrchardNote, WitnessTrees};

use super::notes;

use super::record_book::{NoteRecordIdentifier, RefRecordBook};
use super::traits::SpendableNote;
use super::transaction_context::TransactionContext;
use super::transactions::TxMapAndMaybeTrees;
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

pub fn build_transaction_request_from_receivers(
    receivers: Receivers,
) -> Result<TransactionRequest, Zip321Error> {
    let mut payments = vec![];
    for out in receivers.clone() {
        payments.push(Payment {
            recipient_address: out.0,
            amount: out.1,
            memo: out.2,
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

    /// Propose a transfer, calculate fees, and hold the outgoing transactions. (1, except in the z->tt case where there will be 2.). If a previous transfer was proposed but not sent, it will be overwritten in the buffer.
    pub async fn propose_to_addresses<P: SpendProver + OutputProver>(
        &self,
        sapling_prover: P,
        request: TransactionRequest,
    ) -> ZingoLibResult<Proposal<FeeRule, NoteRecordIdentifier>> {
        let mut context_write_lock: RwLockWriteGuard<'_, TxMapAndMaybeTrees> = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        let mut spend_kit = self.assemble_spend_kit(&mut context_write_lock).await?;
        let extra_proposal = spend_kit.create_proposal(request.clone())?;
        let proposal = spend_kit.create_proposal(request)?;
        spend_kit.create_transactions(sapling_prover, proposal)?;
        Ok(extra_proposal)
    }

    /// Broadcast the most recently proposed transfer.
    pub async fn send_to_addresses<F, Fut>(
        &self,
        submission_height: BlockHeight,
        broadcast_fn: F,
    ) -> Result<Vec<TxId>, SendToAddressesError>
    where
        F: Fn(Box<[u8]>) -> Fut + Clone,
        Fut: Future<Output = Result<String, String>>,
    {
        // review! what does this do? is it necessary? why does it seem wrong?
        {
            self.send_progress.write().await.is_send_in_progress = false;
        }
        // unlock the records. as we translate a newly created raw transaction into a transaction record, broadcast it.
        let lock = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        if let Some(spending_data) = &lock.spending_data {
            let mut sent_txids = Vec::new();
            // send each transaction in the proposition.
            for sending_raw in spending_data.outgoing_send_step_data.iter() {
                // broadcast the raw transaction to the lightserver
                match broadcast_fn(sending_raw.clone().into_boxed_slice()).await {
                    Ok(_txid_string) => (),
                    Err(e) => {
                        return Err(if sent_txids.len() == 0 {
                            SendToAddressesError::NoBroadcast(e)
                        } else {
                            SendToAddressesError::PartialBroadcast(sent_txids, e)
                        })
                    }
                };

                // read the raw transaction
                let sending_full_tx =
                    Transaction::read(&sending_raw[..], zcash_primitives::consensus::BranchId::Nu5)
                        .map_err(|e| SendToAddressesError::Decode(e.to_string()))?;

                // add the sent txid to list
                sent_txids.push(sending_full_tx.txid());

                let price = self.price.read().await.clone();
                let status = ConfirmationStatus::Broadcast(submission_height);
                // and add the sent transaction as a pending (Broadcast) record.
                self.transaction_context
                    .scan_full_tx(
                        &sending_full_tx,
                        status,
                        now() as u32,
                        get_price(now(), &price),
                    )
                    .await;
            }
            if sent_txids.len() == 0 {
                Err(SendToAddressesError::NoProposal)
            } else {
                Ok(sent_txids)
            }
        } else {
            Err(SendToAddressesError::NoSpendCapability)
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
                one_proposal: &mut spending_data.one_proposal,
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

    use super::{build_transaction_request_from_receivers, Receivers};

    #[test]
    fn test_build_request() {
        let amount_1 = NonNegativeAmount::const_from_u64(20000);
        let recipient_address_1 =
            Address::decode(&ChainType::Testnet, &"utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05".to_string()).unwrap();
        let memo_1 = None;

        let amount_2 = NonNegativeAmount::const_from_u64(20000);
        let recipient_address_2 =
            Address::decode(&ChainType::Testnet, &"utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05".to_string()).unwrap();
        let memo_2 = Some(MemoBytes::from(
            Memo::from_str(&"the lake wavers along the beach".to_string())
                .expect("string can memofy"),
        ));

        let rec: Receivers = vec![
            (recipient_address_1, amount_1, memo_1),
            (recipient_address_2, amount_2, memo_2),
        ];
        let request: TransactionRequest =
            build_transaction_request_from_receivers(rec).expect("rec can requestify");

        assert_eq!(
            request.total().expect("total"),
            (amount_1 + amount_2).expect("add")
        );
    }
}

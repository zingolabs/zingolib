//! This mod contains pieces of the impl LightWallet that are invoked during a send.
use crate::wallet::notes::OutputInterface;
use crate::wallet::now;
use crate::{data::receivers::Receivers, wallet::data::SpendableSaplingNote};

use futures::Future;

use log::{error, info};

use orchard::note_encryption::OrchardDomain;

use rand::rngs::OsRng;

use sapling_crypto::note_encryption::SaplingDomain;
use sapling_crypto::prover::{OutputProver, SpendProver};

use orchard::tree::MerkleHashOrchard;
use shardtree::error::{QueryError, ShardTreeError};
use shardtree::store::memory::MemoryShardStore;
use shardtree::ShardTree;
use zcash_note_encryption::Domain;

use std::cmp;
use std::convert::Infallible;
use std::ops::Add;
use std::sync::mpsc::channel;

use zcash_client_backend::{address, zip321::TransactionRequest, PoolType, ShieldedProtocol};
use zcash_keys::address::Address;
use zcash_primitives::transaction::builder::{BuildResult, Progress};
use zcash_primitives::transaction::components::amount::NonNegativeAmount;
use zcash_primitives::transaction::fees::fixed::FeeRule as FixedFeeRule;
use zcash_primitives::transaction::{self, Transaction};
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
use zcash_primitives::{memo::MemoBytes, transaction::TxId};

use zingo_memo::create_wallet_internal_memo_version_0;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::data::witness_trees::{WitnessTrees, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL};

use super::data::SpendableOrchardNote;

use super::notes::ShieldedNoteInterface;
use super::{notes, LightWallet};

use super::traits::{DomainWalletExt, Recipient, SpendableNote};
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
    pub last_error: Option<String>,
    /// TODO: Add Doc Comment Here!
    pub last_transaction_id: Option<String>,
}

impl SendProgress {
    /// TODO: Add Doc Comment Here!
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
    async fn reset_send_progress(&self) {
        let mut g = self.send_progress.write().await;
        let next_id = g.id + 1;

        // Discard the old value, since we are replacing it
        let _ = std::mem::replace(&mut *g, SendProgress::new(next_id));
    }

    /// Get the current sending status.
    pub async fn get_send_progress(&self) -> SendProgress {
        self.send_progress.read().await.clone()
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

        // Add this transaction to the mempool structure
        {
            let price = self.price.read().await.clone();

            let status = ConfirmationStatus::Pending(submission_height);
            self.transaction_context
                .scan_full_tx(transaction, status, now() as u32, get_price(now(), &price))
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

    use zcash_client_backend::{address::Address, zip321::TransactionRequest};
    use zcash_primitives::{
        memo::{Memo, MemoBytes},
        transaction::components::amount::NonNegativeAmount,
    };
    use zingoconfig::ChainType;

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

//! This mod contains pieces of the impl LightWallet that are invoked during a send.

use log::error;
use nonempty::NonEmpty;
use zcash_address::AddressKind;
use zcash_client_backend::proposal::Proposal;
use zcash_primitives::consensus;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::Transaction;
use zcash_primitives::transaction::TxId;
use zcash_proofs::prover::LocalTxProver;

use zcash_client_backend::zip321::TransactionRequest;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::memo::Memo;
use zcash_primitives::memo::MemoBytes;

use pepper_sync::wallet::traits::SyncWallet as _;
use zcash_primitives::transaction::fees::zip317;
use zingo_memo::create_wallet_internal_memo_version_1;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::lightclient::send::send_with_proposal::BroadcastTransactionsError;
use crate::wallet::now;

use super::error::WalletError;
use super::LightWallet;

/// TODO: Add Doc Comment Here!
// TODO: revisit send progress to separate json and handle errors properly
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
    pub last_result: Option<String>,
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

impl From<SendProgress> for json::JsonValue {
    fn from(value: SendProgress) -> Self {
        json::object! {
            "id" => value.id,
            "sending" => value.is_send_in_progress,
            "progress" => value.progress,
            "total" => value.total,
            "last_result" => value.last_result,
        }
    }
}

impl LightWallet {
    // Reset the send progress status to blank
    pub(crate) async fn reset_send_progress(&mut self) {
        let next_id = self.send_progress.id + 1;
        self.send_progress = SendProgress::new(next_id);
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
            WalletError,
            std::convert::Infallible,
            std::convert::Infallible,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
    #[error("Only tex multistep transactions are supported!")]
    NonTexMultiStep,
}

impl LightWallet {
    /// Creates and stores transaction from the given `proposal`, returning the txids for each calculated transaction.
    pub(crate) async fn create_transactions<NoteRef>(
        &mut self,
        proposal: &Proposal<zip317::FeeRule, NoteRef>,
    ) -> Result<NonEmpty<TxId>, BuildTransactionError> {
        if !self.unified_key_store.is_spending_key() {
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
        &mut self,
        sapling_prover: LocalTxProver,
        proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
    ) -> Result<NonEmpty<TxId>, BuildTransactionError> {
        let network = self.network;
        let usk = (&self.unified_key_store)
            .try_into()
            .map_err(BuildTransactionError::UnifiedSpendKey)?;

        Ok(
            zcash_client_backend::data_api::wallet::create_proposed_transactions(
                self,
                &network,
                &sapling_prover,
                &sapling_prover,
                &usk,
                zcash_client_backend::wallet::OvkPolicy::Sender,
                proposal,
                None,
            )?,
        )
    }

    /// Broadcasts calculated transactions stored in the wallet matching txids of `calculated_txids` in the given order.
    /// Rescans each transaction with an updated confirmation status of `Transmitted`, updating spent statuses of all
    /// outputs in the wallet.
    pub(crate) async fn broadcast_calculated_transactions(
        &mut self,
        server_uri: http::Uri,
        calculated_txids: NonEmpty<TxId>,
    ) -> Result<Vec<TxId>, BroadcastTransactionsError> {
        struct SentTransaction {
            txid: TxId,
            height: BlockHeight,
            transaction: Transaction,
        }

        let network = self.network;
        let mut sent_transactions = Vec::new();

        for txid in calculated_txids {
            let calculated_transaction = self
                .wallet_transactions
                .get_mut(&txid)
                .ok_or(BroadcastTransactionsError::TransactionNotFound(txid))?;

            if !matches!(
                calculated_transaction.status(),
                ConfirmationStatus::Calculated(_)
            ) {
                return Err(BroadcastTransactionsError::IncorrectTransactionStatus(txid));
            }

            let height = calculated_transaction.status().get_height();

            let mut transaction_bytes = vec![];
            calculated_transaction
                .transaction()
                .write(&mut transaction_bytes)
                .map_err(|_| BroadcastTransactionsError::TransactionWrite)?;
            let transaction = Transaction::read(
                transaction_bytes.as_slice(),
                consensus::BranchId::for_height(&network, height),
            )
            .map_err(|_| BroadcastTransactionsError::TransactionRead)?;

            let txid_from_server = crate::grpc_connector::send_transaction(
                server_uri.clone(),
                transaction_bytes.into_boxed_slice(),
            )
            .await
            .map_err(BroadcastTransactionsError::Broadcast)?;

            let txid_from_server =
                crate::utils::conversion::txid_from_hex_encoded_str(txid_from_server.as_str())?;

            if txid_from_server != txid {
                #[cfg(not(feature = "darkside_tests"))]
                {
                    return Err(BroadcastTransactionsError::IncorrectTxidFromServer(
                        txid,
                        txid_from_server,
                    ));
                }
                // during darkside tests, the server may report a different txid to the one calculated.
                #[cfg(feature = "darkside_tests")]
                {
                    // TODO:
                }
            }

            sent_transactions.push(SentTransaction {
                txid,
                height,
                transaction,
            });
        }

        let txids = sent_transactions
            .into_iter()
            .map(|sent_transaction| {
                pepper_sync::scan_pending_transaction(
                    &network,
                    &self
                        .get_unified_full_viewing_keys()
                        .map_err(|_| BroadcastTransactionsError::NoViewCapability)?,
                    self,
                    sent_transaction.transaction,
                    ConfirmationStatus::Transmitted(sent_transaction.height),
                    now(),
                )?;

                Ok(sent_transaction.txid)
            })
            .collect::<Result<Vec<TxId>, BroadcastTransactionsError>>()?;

        Ok(txids)
    }
}

// TODO: move to a more suitable place
// TODO: only need to encode highest used refund address index, not all of them
pub(crate) fn change_memo_from_transaction_request(
    request: &TransactionRequest,
    mut refund_address_count: u32,
) -> MemoBytes {
    let mut recipient_uas = Vec::new();
    let mut refund_address_indexes = Vec::new();
    for payment in request.payments().values() {
        match payment.recipient_address().kind() {
            AddressKind::Unified(ua) => {
                if let Ok(ua) = UnifiedAddress::try_from(ua.clone()) {
                    recipient_uas.push(ua);
                }
            }
            AddressKind::Tex(_) => {
                refund_address_indexes.push(refund_address_count);

                refund_address_count += 1;
            }
            _ => (),
        }
    }
    let uas_bytes = match create_wallet_internal_memo_version_1(
        recipient_uas.as_slice(),
        refund_address_indexes.as_slice(),
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

//! This mod contains pieces of the impl LightWallet that are invoked during a send.

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::Transaction;
use zcash_primitives::transaction::TxId;
use zcash_primitives::transaction::fees::zip317;
use zcash_proofs::prover::LocalTxProver;
use zcash_protocol::consensus;
use zcash_protocol::consensus::Parameters;

use super::LightWallet;
use super::error::CalculateTransactionError;
use super::error::TransmissionError;
use crate::wallet::now;
use pepper_sync::wallet::traits::SyncWallet as _;
use zingo_status::confirmation_status::ConfirmationStatus;

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

impl LightWallet {
    /// Creates and stores transaction from the given `proposal`, returning the txids for each calculated transaction.
    pub(crate) async fn calculate_transactions<NoteRef>(
        &mut self,
        proposal: &Proposal<zip317::FeeRule, NoteRef>,
    ) -> Result<NonEmpty<TxId>, CalculateTransactionError<NoteRef>> {
        if !self.unified_key_store.is_spending_key() {
            return Err(CalculateTransactionError::NoSpendCapability);
        }

        // Reset the progress to start. Any errors will get recorded here
        self.reset_send_progress().await;

        let (sapling_output, sapling_spend): (Vec<u8>, Vec<u8>) =
            crate::wallet::utils::read_sapling_params()
                .map_err(CalculateTransactionError::SaplingParams)?;
        let sapling_prover =
            zcash_proofs::prover::LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

        let calculated_txids = match proposal.steps().len() {
            1 => {
                self.create_proposed_transactions(sapling_prover, proposal)
                    .await?
            }
            2 if proposal.steps()[1]
                .transaction_request()
                .payments()
                .values()
                .any(|payment| {
                    matches!(
                        payment
                            .recipient_address()
                            .clone()
                            .convert_if_network::<zcash_keys::address::Address>(
                                self.network.network_type()
                            ),
                        Ok(zcash_keys::address::Address::Tex(_))
                    )
                }) =>
            {
                self.create_proposed_transactions(sapling_prover, proposal)
                    .await?
            }

            _ => return Err(CalculateTransactionError::NonTexMultiStep),
        };
        self.save_required = true;

        Ok(calculated_txids)
    }

    async fn create_proposed_transactions<NoteRef>(
        &mut self,
        sapling_prover: LocalTxProver,
        proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteRef>,
    ) -> Result<NonEmpty<TxId>, CalculateTransactionError<NoteRef>> {
        let network = self.network;
        let usk = (&self.unified_key_store)
            .try_into()
            .map_err(CalculateTransactionError::UnifiedSpendKey)?;

        zcash_client_backend::data_api::wallet::create_proposed_transactions(
            self,
            &network,
            &sapling_prover,
            &sapling_prover,
            &usk,
            zcash_client_backend::wallet::OvkPolicy::Sender,
            proposal,
        )
        .map_err(CalculateTransactionError::Calculation)
    }

    /// Tranmits calculated transactions stored in the wallet matching txids of `calculated_txids` in the given order.
    /// Returns list of txids successfully transmitted.
    ///
    /// Rescans each transaction with an updated confirmation status of `Transmitted`, updating spent statuses of all
    /// outputs in the wallet.
    /// Updates `self.send_progress.last_result` with JSON string of successfully transmitted txids or error message in
    /// case of failure.
    pub(crate) async fn transmit_transactions(
        &mut self,
        server_uri: http::Uri,
        calculated_txids: NonEmpty<TxId>,
    ) -> Result<NonEmpty<TxId>, TransmissionError> {
        match self
            .transmit_transactions_inner(server_uri, calculated_txids)
            .await
        {
            Ok(txids) => {
                self.set_send_result(
                    serde_json::Value::Array(
                        txids
                            .iter()
                            .map(|txid| serde_json::Value::String(txid.to_string()))
                            .collect(),
                    )
                    .to_string(),
                );
                Ok(txids)
            }
            Err(e) => {
                self.set_send_result(format!("error: {e}"));
                Err(e)
            }
        }
    }

    async fn transmit_transactions_inner(
        &mut self,
        server_uri: http::Uri,
        calculated_txids: NonEmpty<TxId>,
    ) -> Result<NonEmpty<TxId>, TransmissionError> {
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
                .ok_or(TransmissionError::TransactionNotFound(txid))?;

            if !matches!(
                calculated_transaction.status(),
                ConfirmationStatus::Calculated(_)
            ) {
                return Err(TransmissionError::IncorrectTransactionStatus(txid));
            }

            let height = calculated_transaction.status().get_height();

            let mut transaction_bytes = vec![];
            calculated_transaction
                .transaction()
                .write(&mut transaction_bytes)
                .map_err(|_| TransmissionError::TransactionWrite)?;
            let transaction = Transaction::read(
                transaction_bytes.as_slice(),
                consensus::BranchId::for_height(&network, height),
            )
            .map_err(|_| TransmissionError::TransactionRead)?;

            let txid_from_server = crate::grpc_connector::send_transaction(
                server_uri.clone(),
                transaction_bytes.into_boxed_slice(),
            )
            .await
            .map_err(TransmissionError::TransmissionFailed)?;

            let txid_from_server =
                crate::utils::conversion::txid_from_hex_encoded_str(txid_from_server.as_str())?;

            if txid_from_server != txid {
                // during darkside tests, the server may report a different txid to the one calculated.
                #[cfg(not(feature = "darkside_tests"))]
                {
                    return Err(TransmissionError::IncorrectTxidFromServer(
                        txid,
                        txid_from_server,
                    ));
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
                        .map_err(|_| TransmissionError::NoViewCapability)?,
                    self,
                    sent_transaction.transaction,
                    ConfirmationStatus::Transmitted(sent_transaction.height),
                    now(),
                )?;

                Ok(sent_transaction.txid)
            })
            .collect::<Result<Vec<TxId>, TransmissionError>>()?;

        Ok(NonEmpty::from_vec(txids).expect("should be non-empty"))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use zcash_address::ZcashAddress;
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_primitives::memo::{Memo, MemoBytes};
    use zcash_protocol::value::Zatoshis;

    use crate::data::receivers::{Receivers, transaction_request_from_receivers};

    #[test]
    fn test_build_request() {
        let amount_1 = Zatoshis::const_from_u64(20000);
        let recipient_address_1 =
            ZcashAddress::try_from_encoded("utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05").unwrap();
        let memo_1 = None;

        let amount_2 = Zatoshis::const_from_u64(20000);
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

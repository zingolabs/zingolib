use crate::error::ZingoLibError;

use crate::wallet::tmamt::TxMapAndMaybeTrees;

use futures::Future;

use log::info;

use sapling_crypto::prover::{OutputProver, SpendProver};

use zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelector;
use zcash_client_backend::keys::UnifiedSpendingKey;
use zcash_client_backend::wallet::OvkPolicy;
use zcash_client_backend::zip321::{Payment, TransactionRequest};
use zcash_primitives::zip32::AccountId;
use zingoconfig::ChainType;

use std::convert::Infallible;
use std::num::NonZeroU32;
use std::ops::Deref;
use std::sync::mpsc::channel;

use zcash_client_backend::ShieldedProtocol;

use zcash_primitives::transaction::builder::{BuildResult, Progress};

use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::fees::zip317::FeeRule as Zip317FeeRule;
use zcash_primitives::transaction::Transaction;

use zingo_status::confirmation_status::ConfirmationStatus;

use super::utils::get_price;
use super::{now, LightWallet, NoteSelectionPolicy, Receivers};

#[derive(Debug, Clone)]
pub struct SendProgress {
    pub id: u32,
    pub is_send_in_progress: bool,
    pub progress: u32,
    pub total: u32,
    pub last_error: Option<String>,
    pub last_transaction_id: Option<String>,
}

impl LightWallet {
    // Get the current sending status.
    pub async fn get_send_progress(&self) -> SendProgress {
        self.send_progress.read().await.clone()
    }

    pub async fn send_to_addresses<F, Fut, P: SpendProver + OutputProver>(
        &self,
        sapling_prover: P,
        policy: NoteSelectionPolicy,
        receivers: Receivers,
        submission_height: BlockHeight,
        broadcast_fn: F,
    ) -> Result<(String, Vec<u8>), String>
    where
        F: Fn(Box<[u8]>) -> Fut,
        Fut: Future<Output = Result<String, String>>,
    {
        // Reset the progress to start. Any errors will get recorded here
        self.reset_send_progress().await;

        // Sanity check that this is a spending wallet.  Why isn't this done earlier?
        if !self.wallet_capability().can_spend_from_all_pools() {
            // Creating transactions in context of all possible combinations
            // of wallet capabilities requires a rigorous case study
            // and can have undesired effects if not implemented properly.
            //
            // Thus we forbid spending for wallets without complete spending capability for now
            return Err("Wallet is in watch-only mode and thus it cannot spend.".to_string());
        }
        // Create the transaction
        let start_time = now();
        dbg!("building_pub_ready_transaction");
        let build_result = self
            .create_publication_ready_transaction(
                submission_height,
                start_time,
                receivers,
                policy,
                sapling_prover,
            )
            .await?;

        // Call the internal function
        match self
            .send_to_addresses_inner(build_result.transaction(), submission_height, broadcast_fn)
            .await
        {
            Ok((transaction_id, raw_transaction)) => {
                self.set_send_success(transaction_id.clone()).await;
                Ok((transaction_id, raw_transaction))
            }
            Err(e) => {
                self.set_send_error(e.to_string()).await;
                Err(e)
            }
        }
    }
    // Reset the send progress status to blank
    async fn reset_send_progress(&self) {
        let mut g = self.send_progress.write().await;
        let next_id = g.id + 1;

        // Discard the old value, since we are replacing it
        let _ = std::mem::replace(&mut *g, SendProgress::new(next_id));
    }

    async fn create_publication_ready_transaction<P: SpendProver + OutputProver>(
        &self,
        submission_height: BlockHeight,
        start_time: u64,
        receivers: Receivers,
        _policy: NoteSelectionPolicy,
        sapling_prover: P,
        // We only care about the transaction...but it can now only be aquired by reference
        // from the build result, so we need to return the whole thing
    ) -> Result<BuildResult, String> {
        // Start building transaction with spends and outputs set by:
        //  * target amount
        //  * selection policy
        //  * recipient list

        // start create_and_populate_tx_builder

        let fee_rule = &Zip317FeeRule::standard(); // Start building tx

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

        let request = TransactionRequest::new(payments).map_err(|e| e.to_string())?;

        dbg!("getting write lock");
        let arc_ledger = self.transactions();
        let mut write_ledger = arc_ledger.write().await;
        let mut ledger = &mut write_ledger;
        let change_strategy = zcash_client_backend::fees::standard::SingleOutputChangeStrategy::new(
            zcash_primitives::transaction::fees::StandardFeeRule::Zip317,
            None,
            ShieldedProtocol::Orchard,
        );
        let input_selector = GreedyInputSelector::<TxMapAndMaybeTrees, _>::new(
            change_strategy,
            zcash_client_backend::fees::DustOutputPolicy::default(),
        );
        let min_confirmations = NonZeroU32::new(1).unwrap();

        dbg!("proposal");
        let proposal = zcash_client_backend::data_api::wallet::propose_transfer::<
            TxMapAndMaybeTrees,
            ChainType,
            GreedyInputSelector<
                TxMapAndMaybeTrees,
                zcash_client_backend::fees::standard::SingleOutputChangeStrategy,
            >,
            ZingoLibError,
        >(
            &mut ledger,
            &self.transaction_context.config.chain,
            zcash_primitives::zip32::AccountId::ZERO,
            &input_selector,
            request,
            min_confirmations,
        )
        .map_err(|e| e.to_string())?;

        let steps = proposal.steps();
        if steps.len() != 1 {
            Err("multi-step proposals not supported")?
        }
        let step = &steps.head;
        let empty_step_results = Vec::with_capacity(1);

        let (mnemonic, _) = self.mnemonic().expect("should have spend capability");
        let seed = mnemonic.to_seed("");
        let account_id = AccountId::ZERO;
        let usk = UnifiedSpendingKey::from_seed(
            &self.transaction_context.config.chain,
            &seed,
            account_id,
        )
        .expect("should be able to create a unified spend key");

        dbg!("calculating");
        let (build_result, _account, _outputs, _utxos_spent) =
            zcash_client_backend::data_api::wallet::calculate_proposed_transaction::<
                TxMapAndMaybeTrees,
                ChainType,
                ZingoLibError,
                Zip317FeeRule,
                u32, // note ref
            >(
                &mut ledger,
                &self.transaction_context.config.chain,
                &sapling_prover,
                &sapling_prover,
                &usk,
                OvkPolicy::Sender,
                fee_rule,
                submission_height,
                &empty_step_results,
                step,
            )
            .map_err(|e| e.to_string())?;

        // old create_and_populate v
        let total_shielded_receivers = 0;
        // end create_and_populate_tx_builder

        // The builder now has the correct set of inputs and outputs

        // Set up a channel to receive updates on the progress of building the transaction.
        // This progress monitor, the channel monitoring it, and the types necessary for its
        // construction are unnecessary for sending.
        let (_transmitter, receiver) = channel::<Progress>();
        let progress = self.send_progress.clone();

        // Use a separate thread to handle sending from std::mpsc to tokio::sync::mpsc
        let (transmitter2, mut receiver2) = tokio::sync::mpsc::unbounded_channel();
        std::thread::spawn(move || {
            while let Ok(r) = receiver.recv() {
                transmitter2.send(r.cur()).unwrap();
            }
        });

        let progress_handle = tokio::spawn(async move {
            while let Some(r) = receiver2.recv().await {
                info!("{}: Progress: {r}", now() - start_time);
                progress.write().await.progress = r;
            }

            progress.write().await.is_send_in_progress = false;
        });

        {
            let mut p = self.send_progress.write().await;
            p.is_send_in_progress = true;
            p.progress = 0;
            p.total = total_shielded_receivers;
        }

        info!("{}: Building transaction", now() - start_time);

        // let tx_builder = tx_builder.with_progress_notifier(transmitter);
        // let build_result = match tx_builder.build(
        //     OsRng,
        //     &sapling_prover,
        //     &sapling_prover,
        //     &transaction::fees::fixed::FeeRule::non_standard(MINIMUM_FEE),
        // ) {
        //     Ok(res) => res,
        //     Err(e) => {
        //         let e = format!("Error creating transaction: {:?}", e);
        //         error!("{}", e);
        //         self.send_progress.write().await.is_send_in_progress = false;
        //         return Err(e);
        //     }
        // };
        progress_handle.await.unwrap();
        Ok(build_result)
    }

    /// This function takes a already-calculated transaction and does 2 steps with it:
    /// broadcasts it
    /// records it to the internal ledger
    async fn send_to_addresses_inner<F, Fut>(
        &self,
        transaction: &Transaction,
        submission_height: BlockHeight,
        broadcast_fn: F,
    ) -> Result<(String, Vec<u8>), String>
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

        let transaction_id = broadcast_fn(raw_transaction.clone().into_boxed_slice()).await?;

        // Add this transaction to the mempool structure
        {
            let price = self.price.read().await.clone();

            let status = ConfirmationStatus::Broadcast(submission_height);
            self.transaction_context
                .scan_full_tx(transaction, status, now() as u32, get_price(now(), &price))
                .await;
        }

        Ok((transaction_id, raw_transaction))
    }
}

#[cfg(test)]
mod tests {

    // #[test]
    // fn test_send_to_addresses() {
    //     let change_strategy = zcash_client_backend::fees::standard::SingleOutputChangeStrategy::new(
    //         zcash_primitives::transaction::fees::StandardFeeRule::Zip317,
    //         None,
    //         ShieldedProtocol::Orchard,
    //     );
    //     let input_selector = GreedyInputSelector::<ZingoLedger, _>::new(
    //         change_strategy,
    //         DustOutputPolicy::default(),
    //     );

    //     let min_confirmations = NonZeroU32::new(10).unwrap();

    //     let mut ledger = ZingoLedger::new_treeless();
    //     assert_eq!(
    //         ledger
    //             .get_target_and_anchor_heights(min_confirmations)
    //             .unwrap(),
    //         None
    //     );

    //     dbg!("inventing transaction");
    //     let txid = TxId::from_bytes([0; 32]);
    //     let status = ConfirmationStatus::Confirmed(BlockHeight::from(100));
    //     let timestamp = 123456;

    //     dbg!("inventing nullifier");
    //     let nullifier = Nullifier::from_bytes(&[0; 32]).unwrap();
    //     dbg!("inventing address");
    //     let internal_orchard_address = orchard::Address::from_raw_address_bytes(&[7; 43]).unwrap(); //this vector of 7s happens to work
    //     dbg!("inventing note");
    //     let note = {
    //         let random_seed = RandomSeed::from_bytes([0; 32], &nullifier).unwrap();
    //         orchard::note::Note::from_parts(
    //             internal_orchard_address,
    //             NoteValue::from_raw(40000),
    //             nullifier,
    //             random_seed,
    //         )
    //         .unwrap()
    //     };
    //     dbg!("adding note");
    //     let position = Position::from(100);
    //     ledger.add_new_note::<OrchardDomain>(
    //         txid,
    //         status,
    //         timestamp,
    //         note,
    //         internal_orchard_address,
    //         true,
    //         Some(nullifier),
    //         0,
    //         position,
    //     );

    //     let request_amount = NonNegativeAmount::const_from_u64(20000);
    //     let recipient_address =
    //         Address::decode(&ChainType::Testnet, &"utest17wwv8nuvdnpjsxtu6ndz6grys5x8wphcwtzmg75wkx607c7cue9qz5kfraqzc7k9dfscmylazj4nkwazjj26s9rhyjxm0dcqm837ykgh2suv0at9eegndh3kvtfjwp3hhhcgk55y9d2ys56zkw8aaamcrv9cy0alj0ndvd0wll4gxhrk9y4yy9q9yg8yssrencl63uznqnkv7mk3w05".to_string()).unwrap();
    //     let request = zip321::TransactionRequest::new(vec![Payment {
    //         recipient_address,
    //         amount: request_amount,
    //         memo: None,
    //         label: None,
    //         message: None,
    //         other_params: vec![],
    //     }])
    //     .expect(
    //         "It should not be possible for this to violate ZIP 321 request construction invariants.",);

    //     dbg!("proposing transfer");
    // }
}

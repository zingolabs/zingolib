//! This mod contains pieces of the impl LightWallet that are invoked during a send.
use crate::wallet::now;
use crate::wallet::{notes::OutputInterface, traits::Bundle};
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
use super::{notes, traits, LightWallet};

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

pub(crate) type NoteSelectionPolicy = Vec<PoolType>;

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

fn add_notes_to_total<D: DomainWalletExt>(
    candidates: Vec<D::SpendableNoteAT>,
    target_amount: Amount,
) -> (Vec<D::SpendableNoteAT>, Amount)
where
    D::Note: PartialEq + Clone,
    D::Recipient: traits::Recipient,
{
    let mut notes = Vec::new();
    let mut running_total = Amount::zero();
    for note in candidates {
        if running_total >= target_amount {
            break;
        }
        running_total = running_total
            .add(
                Amount::from_u64(<D as DomainWalletExt>::WalletNote::value_from_note(
                    note.note(),
                ))
                .expect("should be within the valid monetary range of zatoshis"),
            )
            .expect("should be within the valid monetary range of zatoshis");
        notes.push(note);
    }

    (notes, running_total)
}

type TxBuilder<'a> = Builder<'a, zingoconfig::ChainType, ()>;
impl LightWallet {
    pub(super) async fn get_orchard_anchor(
        &self,
        tree: &ShardTree<
            MemoryShardStore<MerkleHashOrchard, BlockHeight>,
            COMMITMENT_TREE_LEVELS,
            MAX_SHARD_LEVEL,
        >,
    ) -> Result<orchard::Anchor, ShardTreeError<Infallible>> {
        Ok(orchard::Anchor::from(tree.root_at_checkpoint_depth(
            self.transaction_context.config.reorg_buffer_offset as usize,
        )?))
    }

    pub(super) async fn get_sapling_anchor(
        &self,
        tree: &ShardTree<
            MemoryShardStore<sapling_crypto::Node, BlockHeight>,
            COMMITMENT_TREE_LEVELS,
            MAX_SHARD_LEVEL,
        >,
    ) -> Result<sapling_crypto::Anchor, ShardTreeError<Infallible>> {
        Ok(sapling_crypto::Anchor::from(
            tree.root_at_checkpoint_depth(
                self.transaction_context.config.reorg_buffer_offset as usize,
            )?,
        ))
    }

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

    /// TODO: Add Doc Comment Here!
    pub async fn send_to_addresses<F, Fut, P: SpendProver + OutputProver>(
        &self,
        sapling_prover: P,
        policy: NoteSelectionPolicy,
        receivers: Receivers,
        submission_height: BlockHeight,
        broadcast_fn: F,
    ) -> Result<TxId, String>
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
        dbg!(&build_result
            .transaction()
            .orchard_bundle()
            .unwrap()
            .output_elements()
            .len());
        match self
            .send_to_addresses_inner(build_result.transaction(), submission_height, broadcast_fn)
            .await
        {
            Ok(transaction_id) => {
                self.set_send_success(transaction_id.to_string()).await;
                Ok(transaction_id)
            }
            Err(e) => {
                self.set_send_error(e.to_string()).await;
                Err(e)
            }
        }
    }

    async fn create_publication_ready_transaction<P: SpendProver + OutputProver>(
        &self,
        submission_height: BlockHeight,
        start_time: u64,
        receivers: Receivers,
        policy: NoteSelectionPolicy,
        sapling_prover: P,
        // We only care about the transaction...but it can now only be aquired by reference
        // from the build result, so we need to return the whole thing
    ) -> Result<BuildResult, String> {
        // Start building transaction with spends and outputs set by:
        //  * target amount
        //  * selection policy
        //  * recipient list
        let txmds_readlock = self
            .transaction_context
            .transaction_metadata_set
            .read()
            .await;
        let witness_trees = txmds_readlock
            .witness_trees()
            .ok_or("No spend capability.")?;
        let (tx_builder, total_shielded_receivers) = match self
            .create_and_populate_tx_builder(
                submission_height,
                witness_trees,
                start_time,
                receivers,
                policy,
            )
            .await
        {
            Ok(tx_builder) => tx_builder,
            Err(s) => {
                return Err(s);
            }
        };

        drop(txmds_readlock);
        // The builder now has the correct set of inputs and outputs

        // Set up a channel to receive updates on the progress of building the transaction.
        // This progress monitor, the channel monitoring it, and the types necessary for its
        // construction are unnecessary for sending.
        let (transmitter, receiver) = channel::<Progress>();
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

        let tx_builder = tx_builder.with_progress_notifier(transmitter);
        let build_result = match tx_builder.build(
            OsRng,
            &sapling_prover,
            &sapling_prover,
            &transaction::fees::fixed::FeeRule::non_standard(MINIMUM_FEE),
        ) {
            Ok(res) => res,
            Err(e) => {
                let e = format!("Error creating transaction: {:?}", e);
                error!("{}", e);
                self.send_progress.write().await.is_send_in_progress = false;
                return Err(e);
            }
        };
        progress_handle.await.unwrap();
        Ok(build_result)
    }

    async fn create_and_populate_tx_builder(
        &self,
        submission_height: BlockHeight,
        witness_trees: &WitnessTrees,
        start_time: u64,
        receivers: Receivers,
        policy: NoteSelectionPolicy,
    ) -> Result<(TxBuilder<'_>, u32), String> {
        let fee_rule =
            &zcash_primitives::transaction::fees::fixed::FeeRule::non_standard(MINIMUM_FEE); // Start building tx
        let mut total_shielded_receivers;
        let mut orchard_notes;
        let mut sapling_notes;
        let mut utxos;
        let mut tx_builder;
        let mut proposed_fee = MINIMUM_FEE;
        let mut total_value_covered_by_selected;
        let total_earmarked_for_recipients: u64 =
            receivers.iter().map(|to| u64::from(to.amount)).sum();
        info!(
            "0: Creating transaction sending {} zatoshis to {} addresses",
            total_earmarked_for_recipients,
            receivers.len()
        );
        loop {
            tx_builder = match self
                .create_tx_builder(submission_height, witness_trees)
                .await
            {
                Err(ShardTreeError::Query(QueryError::NotContained(addr))) => Err(format!(
                    "could not create anchor, missing address {addr:?}. \
                    If you are fully synced, you may need to rescan to proceed"
                )),
                Err(ShardTreeError::Query(QueryError::CheckpointPruned)) => {
                    let blocks = self.blocks.read().await.len();
                    let offset = self.transaction_context.config.reorg_buffer_offset;
                    Err(format!(
                        "The reorg buffer offset has been set to {} \
                        but there are only {} blocks in the wallet. \
                        Please sync at least {} more blocks before trying again",
                        offset,
                        blocks,
                        offset + 1 - blocks as u32
                    ))
                }
                Err(ShardTreeError::Query(QueryError::TreeIncomplete(addrs))) => Err(format!(
                    "could not create anchor, missing addresses {addrs:?}. \
                    If you are fully synced, you may need to rescan to proceed"
                )),
                Err(ShardTreeError::Insert(_)) => unreachable!(),
                Err(ShardTreeError::Storage(_infallible)) => unreachable!(),
                Ok(v) => Ok(v),
            }?;

            // Select notes to cover the target value
            info!("{}: Adding outputs", now() - start_time);
            (total_shielded_receivers, tx_builder) = self
                .add_consumer_specified_outputs_to_builder(tx_builder, receivers.clone())
                .expect("To add outputs");

            let earmark_total_plus_default_fee =
                total_earmarked_for_recipients + u64::from(proposed_fee);
            // Select notes as a fn of target amount
            (
                orchard_notes,
                sapling_notes,
                utxos,
                total_value_covered_by_selected,
            ) = match self
                .select_notes_and_utxos(
                    Amount::from_u64(earmark_total_plus_default_fee)
                        .expect("Valid amount, from u64."),
                    &policy,
                )
                .await
            {
                Ok(notes) => notes,
                Err(insufficient_amount) => {
                    let e = format!(
                "Insufficient verified shielded funds. Have {} zats, need {} zats. NOTE: funds need at least {} confirmations before they can be spent. Transparent funds must be shielded before they can be spent. If you are trying to spend transparent funds, please use the shield button and try again in a few minutes.",
                insufficient_amount, earmark_total_plus_default_fee, self.transaction_context.config
                .reorg_buffer_offset + 1
            );
                    error!("{}", e);
                    return Err(e);
                }
            };

            info!("Selected notes worth {}", total_value_covered_by_selected);

            info!(
                "{}: Adding {} sapling notes, {} orchard notes, and {} utxos",
                now() - start_time,
                &sapling_notes.len(),
                &orchard_notes.len(),
                &utxos.len()
            );

            let temp_tx_builder = match self.add_change_output_to_builder(
                tx_builder,
                Amount::from_u64(earmark_total_plus_default_fee).expect("valid value of u64"),
                Amount::from_u64(total_value_covered_by_selected).unwrap(),
                &mut total_shielded_receivers,
                &receivers,
            ) {
                Ok(txb) => txb,
                Err(r) => {
                    return Err(r);
                }
            };
            info!("{}: selecting notes", now() - start_time);
            tx_builder = match self
                .add_spends_to_builder(
                    temp_tx_builder,
                    witness_trees,
                    &orchard_notes,
                    &sapling_notes,
                    &utxos,
                )
                .await
            {
                Ok(tx_builder) => tx_builder,

                Err(s) => {
                    return Err(s);
                }
            };
            proposed_fee = tx_builder.get_fee(fee_rule).unwrap();
            if u64::from(proposed_fee) + total_earmarked_for_recipients
                <= total_value_covered_by_selected
            {
                break;
            }
        }
        Ok((tx_builder, total_shielded_receivers))
    }

    async fn create_tx_builder(
        &self,
        submission_height: BlockHeight,
        witness_trees: &WitnessTrees,
    ) -> Result<TxBuilder, ShardTreeError<Infallible>> {
        let orchard_anchor = self
            .get_orchard_anchor(&witness_trees.witness_tree_orchard)
            .await?;
        let sapling_anchor = self
            .get_sapling_anchor(&witness_trees.witness_tree_sapling)
            .await?;
        Ok(Builder::new(
            self.transaction_context.config.chain,
            submission_height,
            transaction::builder::BuildConfig::Standard {
                // TODO: We probably need this
                sapling_anchor: Some(sapling_anchor),
                orchard_anchor: Some(orchard_anchor),
            },
        ))
    }

    fn add_consumer_specified_outputs_to_builder<'a>(
        &'a self,
        mut tx_builder: TxBuilder<'a>,
        receivers: Receivers,
    ) -> Result<(u32, TxBuilder<'_>), String> {
        // Convert address (str) to RecipientAddress and value to Amount

        // We'll use the first ovk to encrypt outgoing transactions
        let sapling_ovk =
            sapling_crypto::keys::OutgoingViewingKey::try_from(&*self.wallet_capability()).unwrap();
        let orchard_ovk =
            orchard::keys::OutgoingViewingKey::try_from(&*self.wallet_capability()).unwrap();

        let mut total_shielded_receivers = 0u32;
        for crate::data::receivers::Receiver {
            recipient_address,
            amount,
            memo,
        } in receivers
        {
            // Compute memo if it exists
            let validated_memo = match memo {
                None => MemoBytes::from(Memo::Empty),
                Some(s) => s,
            };

            if let Err(e) = match recipient_address {
                address::Address::Transparent(to) => tx_builder
                    .add_transparent_output(&to, amount)
                    .map_err(transaction::builder::Error::TransparentBuild),
                address::Address::Sapling(to) => {
                    total_shielded_receivers += 1;
                    tx_builder.add_sapling_output(Some(sapling_ovk), to, amount, validated_memo)
                }
                address::Address::Unified(ua) => {
                    if let Some(orchard_addr) = ua.orchard() {
                        total_shielded_receivers += 1;
                        tx_builder.add_orchard_output::<FixedFeeRule>(
                            Some(orchard_ovk.clone()),
                            *orchard_addr,
                            u64::from(amount),
                            validated_memo,
                        )
                    } else if let Some(sapling_addr) = ua.sapling() {
                        total_shielded_receivers += 1;
                        tx_builder.add_sapling_output(
                            Some(sapling_ovk),
                            *sapling_addr,
                            amount,
                            validated_memo,
                        )
                    } else {
                        return Err("Received UA with no Orchard or Sapling receiver".to_string());
                    }
                }
            } {
                let e = format!("Error adding output: {:?}", e);
                error!("{}", e);
                return Err(e);
            }
        }
        Ok((total_shielded_receivers, tx_builder))
    }

    async fn get_all_domain_specific_notes<D>(&self) -> Vec<D::SpendableNoteAT>
    where
        D: DomainWalletExt,
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        let wc = self.wallet_capability();
        let tranmds_lth = self.transactions();
        let transaction_metadata_set = tranmds_lth.read().await;
        let mut candidate_notes = transaction_metadata_set
            .transaction_records_by_id
            .iter()
            .flat_map(|(transaction_id, transaction)| {
                D::WalletNote::transaction_metadata_notes(transaction)
                    .iter()
                    .map(move |note| (*transaction_id, note))
            })
            .filter_map(
                |(transaction_id, note): (transaction::TxId, &D::WalletNote)| -> Option <D::SpendableNoteAT> {
                        // Get the spending key for the selected fvk, if we have it
                        let extsk = D::wc_to_sk(&wc);
                        SpendableNote::from(transaction_id, note, extsk.ok().as_ref())
                }
            )
            .collect::<Vec<D::SpendableNoteAT>>();
        candidate_notes.sort_unstable_by(|spendable_note_1, spendable_note_2| {
            D::WalletNote::value_from_note(spendable_note_2.note())
                .cmp(&D::WalletNote::value_from_note(spendable_note_1.note()))
        });
        candidate_notes
    }

    async fn select_notes_and_utxos(
        &self,
        target_amount: Amount,
        policy: &NoteSelectionPolicy,
    ) -> Result<
        (
            Vec<SpendableOrchardNote>,
            Vec<SpendableSaplingNote>,
            Vec<notes::TransparentOutput>,
            u64,
        ),
        u64,
    > {
        let mut all_transparent_value_in_wallet = Amount::zero();
        let mut utxos = Vec::new(); //utxo stands for Unspent Transaction Output
        let mut sapling_value_selected = Amount::zero();
        let mut sapling_notes = Vec::new();
        let mut orchard_value_selected = Amount::zero();
        let mut orchard_notes = Vec::new();
        // Correctness of this loop depends on:
        //    * uniqueness
        for pool in policy {
            match pool {
                // Transparent: This opportunistic shielding sweeps all transparent value leaking identifying information to
                // a funder of the wallet's transparent value. We should change this.
                PoolType::Transparent => {
                    utxos = self
                        .get_utxos()
                        .await
                        .iter()
                        .filter(|utxo| utxo.pending_spent.is_none() && !utxo.is_spent())
                        .cloned()
                        .collect::<Vec<_>>();
                    all_transparent_value_in_wallet =
                        utxos.iter().fold(Amount::zero(), |prev, utxo| {
                            (prev + Amount::from_u64(utxo.value).unwrap()).unwrap()
                        });
                }
                PoolType::Shielded(ShieldedProtocol::Sapling) => {
                    let sapling_candidates = self
                        .get_all_domain_specific_notes::<SaplingDomain>()
                        .await
                        .into_iter()
                        .filter(|note| note.spend_key().is_some())
                        .collect();
                    (sapling_notes, sapling_value_selected) = add_notes_to_total::<SaplingDomain>(
                        sapling_candidates,
                        (target_amount - orchard_value_selected - all_transparent_value_in_wallet)
                            .unwrap(),
                    );
                }
                PoolType::Shielded(ShieldedProtocol::Orchard) => {
                    let orchard_candidates = self
                        .get_all_domain_specific_notes::<OrchardDomain>()
                        .await
                        .into_iter()
                        .filter(|note| note.spend_key().is_some())
                        .collect();
                    (orchard_notes, orchard_value_selected) = add_notes_to_total::<OrchardDomain>(
                        orchard_candidates,
                        (target_amount - all_transparent_value_in_wallet - sapling_value_selected)
                            .unwrap(),
                    );
                }
            }
            // Check how much we've selected
            if (all_transparent_value_in_wallet + sapling_value_selected + orchard_value_selected)
                .unwrap()
                >= target_amount
            {
                return Ok((
                    orchard_notes,
                    sapling_notes,
                    utxos,
                    u64::try_from(
                        (all_transparent_value_in_wallet
                            + sapling_value_selected
                            + orchard_value_selected)
                            .unwrap(),
                    )
                    .expect("u64 representable."),
                ));
            }
        }

        // If we can't select enough, then we need to return empty handed
        Err(u64::try_from(
            (all_transparent_value_in_wallet + sapling_value_selected + orchard_value_selected)
                .unwrap(),
        )
        .expect("u64 representable"))
    }

    // TODO: LEGACY. to be deprecated when zip317 lands
    fn add_change_output_to_builder<'a>(
        &self,
        mut tx_builder: TxBuilder<'a>,
        target_amount: Amount,
        selected_value: Amount,
        total_shielded_receivers: &mut u32,
        receivers: &Receivers,
    ) -> Result<TxBuilder<'a>, String> {
        let destination_uas = receivers
            .iter()
            .filter_map(|receiver| match receiver.recipient_address {
                address::Address::Sapling(_) => None,
                address::Address::Transparent(_) => None,
                address::Address::Unified(ref ua) => Some(ua.clone()),
            })
            .collect::<Vec<_>>();
        let uas_bytes = match create_wallet_internal_memo_version_0(destination_uas.as_slice()) {
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
        let orchard_ovk =
            orchard::keys::OutgoingViewingKey::try_from(&*self.wallet_capability()).unwrap();
        *total_shielded_receivers += 1;
        if let Err(e) = tx_builder.add_orchard_output::<FixedFeeRule>(
            Some(orchard_ovk.clone()),
            *self.wallet_capability().addresses()[0].orchard().unwrap(),
            u64::try_from(selected_value).expect("u64 representable")
                - u64::try_from(target_amount).expect("u64 representable"),
            // Here we store the uas we sent to in the memo field.
            // These are used to recover the full UA we sent to.
            MemoBytes::from(Memo::Arbitrary(Box::new(uas_bytes))),
        ) {
            let e = format!("Error adding change output: {:?}", e);
            error!("{}", e);
            return Err(e);
        };
        Ok(tx_builder)
    }

    async fn add_spends_to_builder<'a>(
        &'a self,
        mut tx_builder: TxBuilder<'a>,
        witness_trees: &WitnessTrees,
        orchard_notes: &[SpendableOrchardNote],
        sapling_notes: &[SpendableSaplingNote],
        utxos: &[notes::TransparentOutput],
    ) -> Result<TxBuilder<'_>, String> {
        // Add all tinputs
        // Create a map from address -> sk for all taddrs, so we can spend from the
        // right address
        let address_to_sk = self
            .wallet_capability()
            .get_taddr_to_secretkey_map(&self.transaction_context.config)
            .unwrap();

        utxos
            .iter()
            .map(|utxo| {
                let outpoint: OutPoint = utxo.to_outpoint();

                let coin = TxOut {
                    value: NonNegativeAmount::from_u64(utxo.value).unwrap(),
                    script_pubkey: Script(utxo.script.clone()),
                };

                match address_to_sk.get(&utxo.address) {
                    Some(sk) => tx_builder
                        .add_transparent_input(*sk, outpoint, coin)
                        .map_err(|e| {
                            transaction::builder::Error::<Infallible>::TransparentBuild(e)
                        }),
                    None => {
                        // Something is very wrong
                        let e = format!("Couldn't find the secretkey for taddr {}", utxo.address);
                        error!("{}", e);

                        Err(transaction::builder::Error::<Infallible>::TransparentBuild(
                            transaction::components::transparent::builder::Error::InvalidAddress,
                        ))
                    }
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("{:?}", e))?;

        for selected in sapling_notes.iter() {
            info!("Adding sapling spend");
            // Turbofish only needed for error type
            if let Err(e) = tx_builder.add_sapling_spend::<FixedFeeRule>(
                &selected.extsk.clone().unwrap(),
                selected.note.clone(),
                witness_trees
                    .witness_tree_sapling
                    .witness_at_checkpoint_depth(
                        selected.witnessed_position,
                        self.transaction_context.config.reorg_buffer_offset as usize,
                    )
                    .map_err(|e| format!("failed to compute sapling witness: {e}"))?,
            ) {
                let e = format!("Error adding note: {:?}", e);
                error!("{}", e);
                return Err(e);
            }
        }

        for selected in orchard_notes.iter() {
            info!("Adding orchard spend");
            if let Err(e) = tx_builder.add_orchard_spend::<transaction::fees::fixed::FeeRule>(
                &selected.spend_key.unwrap(),
                selected.note,
                orchard::tree::MerklePath::from(
                    witness_trees
                        .witness_tree_orchard
                        .witness_at_checkpoint_depth(
                            selected.witnessed_position,
                            self.transaction_context.config.reorg_buffer_offset as usize,
                        )
                        .map_err(|e| format!("failed to compute orchard witness: {e}"))?,
                ),
            ) {
                let e = format!("Error adding note: {:?}", e);
                error!("{}", e);
                return Err(e);
            }
        }
        Ok(tx_builder)
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
            dbg!("About to call scan_full_tx with pending tx");
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
#[cfg(feature = "zip317")]
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

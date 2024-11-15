//! The zcash blockchain and proxied transformations derived from it, do not contain any public
//! information about transaction recipients. Therefore clients must attempt to decrypt each
//! note with each of their keys to determine if they are the recipient.
//! This process is called: `trial_decryption`.

use crate::error::ZingoLibResult;

use crate::config::ZingoConfig;
use crate::wallet::keys::unified::{External, Fvk as _, Ivk};
use crate::wallet::notes::ShieldedNoteInterface;
use crate::wallet::{
    data::PoolNullifier,
    keys::unified::WalletCapability,
    traits::{CompactOutput as _, DomainWalletExt, FromCommitment, Recipient},
    tx_map::TxMap,
    utils::txid_from_slice,
    MemoDownloadOption,
};
use futures::{stream::FuturesUnordered, StreamExt};
use incrementalmerkletree::{Position, Retention};
use log::debug;
use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use std::sync::Arc;
use tokio::{
    sync::{
        mpsc::{unbounded_channel, UnboundedSender},
        oneshot, RwLock,
    },
    task::JoinHandle,
};
use zcash_client_backend::proto::compact_formats::{CompactBlock, CompactTx};
use zcash_note_encryption::Domain;
use zcash_primitives::{
    consensus::{BlockHeight, Parameters},
    transaction::{Transaction, TxId},
};
use zingo_status::confirmation_status::ConfirmationStatus;

use super::syncdata::BlazeSyncData;

pub struct TrialDecryptions {
    wc: Arc<WalletCapability>,
    transaction_metadata_set: Arc<RwLock<TxMap>>,
    config: Arc<ZingoConfig>,
}

impl TrialDecryptions {
    pub fn new(
        config: Arc<ZingoConfig>,
        wc: Arc<WalletCapability>,
        transaction_metadata_set: Arc<RwLock<TxMap>>,
    ) -> Self {
        Self {
            config,
            wc,
            transaction_metadata_set,
        }
    }

    /// Pass keys and data store to dedicated trial_decrpytion *management* thread,
    /// the *management* thread in turns spawns per-1000-cb trial decryption threads.
    pub async fn start(
        &self,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        detected_transaction_id_sender: UnboundedSender<(
            TxId,
            PoolNullifier,
            BlockHeight,
            Option<u32>,
            bool,
        )>,
        transaction_size_filter: Option<u32>,
        full_transaction_fetcher: UnboundedSender<(
            TxId,
            oneshot::Sender<Result<Transaction, String>>,
        )>,
    ) -> (JoinHandle<()>, UnboundedSender<CompactBlock>) {
        //debug!("Starting trial decrptions processor");

        // Create a new channel where we'll receive the blocks
        let (transmitter, mut receiver) = unbounded_channel::<CompactBlock>();

        let wc = self.wc.clone();
        let transaction_metadata_set = self.transaction_metadata_set.clone();

        let config = self.config.clone();
        let management_thread_handle = tokio::spawn(async move {
            let mut workers = FuturesUnordered::new();
            let mut cbs = vec![];

            let sapling_ivk = sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(
                wc.unified_key_store(),
            )
            .ok()
            .map(|key| key.derive_ivk());
            let orchard_ivk = orchard::keys::FullViewingKey::try_from(wc.unified_key_store())
                .ok()
                .map(|key| key.derive_ivk());

            while let Some(cb) = receiver.recv().await {
                cbs.push(cb);
            }
            // Finish off the remaining < 125 cbs
            workers.push(tokio::spawn(Self::trial_decrypt_batch(
                config,
                cbs,
                wc,
                bsync_data,
                sapling_ivk,
                orchard_ivk,
                transaction_metadata_set,
                transaction_size_filter,
                detected_transaction_id_sender,
                full_transaction_fetcher,
            )));

            while let Some(r) = workers.next().await {
                r.unwrap().unwrap();
            }
        });

        (management_thread_handle, transmitter)
    }

    /// Trial decryption is invoked by spend-or-view key holders to detect
    /// notes addressed to shielded addresses derived from the spend key.
    /// In the case that the User has requested Memos, and transaction_metadata
    /// remains false throughout domain-specific decryptions, a memo-specific
    /// thread is spawned.
    #[allow(clippy::too_many_arguments)]
    async fn trial_decrypt_batch(
        config: Arc<ZingoConfig>,
        compact_blocks: Vec<CompactBlock>,
        wc: Arc<WalletCapability>,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        sapling_ivk: Option<Ivk<SaplingDomain, External>>,
        orchard_ivk: Option<Ivk<OrchardDomain, External>>,
        transaction_metadata_set: Arc<RwLock<TxMap>>,
        transaction_size_filter: Option<u32>,
        detected_transaction_id_sender: UnboundedSender<(
            TxId,
            PoolNullifier,
            BlockHeight,
            Option<u32>,
            bool,
        )>,
        full_transaction_fetcher: UnboundedSender<(
            TxId,
            oneshot::Sender<Result<Transaction, String>>,
        )>,
    ) -> Result<(), String> {
        let mut workers = FuturesUnordered::new();

        let download_memos = bsync_data.read().await.wallet_options.download_memos;
        let mut sapling_notes_to_mark_position = Vec::new();
        let mut orchard_notes_to_mark_position = Vec::new();

        for compact_block in compact_blocks {
            let height = BlockHeight::from_u32(compact_block.height as u32);
            let mut sapling_notes_to_mark_position_in_block = Vec::new();
            let mut orchard_notes_to_mark_position_in_block = Vec::new();

            for (transaction_num, compact_transaction) in compact_block.vtx.iter().enumerate() {
                let mut sapling_notes_to_mark_position_in_tx =
                    zip_outputs_with_retention_txids_indexes::<SaplingDomain>(compact_transaction);
                let mut orchard_notes_to_mark_position_in_tx =
                    zip_outputs_with_retention_txids_indexes::<OrchardDomain>(compact_transaction);

                if let Some(filter) = transaction_size_filter {
                    if compact_transaction.outputs.len() + compact_transaction.actions.len()
                        > filter as usize
                    {
                        sapling_notes_to_mark_position_in_block
                            .extend_from_slice(&sapling_notes_to_mark_position_in_tx);
                        orchard_notes_to_mark_position_in_block
                            .extend_from_slice(&orchard_notes_to_mark_position_in_tx);
                        continue;
                    }
                }
                let mut transaction_metadata = false;
                if let Some(ref ivk) = sapling_ivk {
                    Self::trial_decrypt_domain_specific_outputs::<SaplingDomain>(
                        &mut transaction_metadata,
                        compact_transaction,
                        transaction_num,
                        &compact_block,
                        ivk.ivk.clone(),
                        height,
                        &config,
                        &wc,
                        &bsync_data,
                        &transaction_metadata_set,
                        &detected_transaction_id_sender,
                        &workers,
                        &mut sapling_notes_to_mark_position_in_tx,
                    )
                };

                sapling_notes_to_mark_position_in_block
                    .extend_from_slice(&sapling_notes_to_mark_position_in_tx);
                if let Some(ref ivk) = orchard_ivk {
                    Self::trial_decrypt_domain_specific_outputs::<OrchardDomain>(
                        &mut transaction_metadata,
                        compact_transaction,
                        transaction_num,
                        &compact_block,
                        ivk.ivk.clone(),
                        height,
                        &config,
                        &wc,
                        &bsync_data,
                        &transaction_metadata_set,
                        &detected_transaction_id_sender,
                        &workers,
                        &mut orchard_notes_to_mark_position_in_tx,
                    )
                };
                orchard_notes_to_mark_position_in_block
                    .extend_from_slice(&orchard_notes_to_mark_position_in_tx);

                // Check option to see if we are fetching all transactions.
                // grabs all memos regardless of decryption status.
                // Note the memos are immediately discarded.
                // Perhaps this obfuscates the memos of interest?
                if !transaction_metadata && download_memos == MemoDownloadOption::AllMemos {
                    let transaction_id = txid_from_slice(&compact_transaction.hash);
                    let (transmitter, receiver) = oneshot::channel();
                    full_transaction_fetcher
                        .send((transaction_id, transmitter))
                        .unwrap();

                    workers.push(tokio::spawn(async move {
                        // Discard the result, because this was not a wallet transaction.
                        receiver.await.unwrap().map(|_r| ())
                    }));
                }
            }
            sapling_notes_to_mark_position.push((sapling_notes_to_mark_position_in_block, height));
            orchard_notes_to_mark_position.push((orchard_notes_to_mark_position_in_block, height));
        }

        while let Some(r) = workers.next().await {
            r.map_err(|e| e.to_string())??;
        }
        let mut txmds_writelock = transaction_metadata_set.write().await;
        update_witnesses::<SaplingDomain>(
            sapling_notes_to_mark_position,
            &mut txmds_writelock,
            &wc,
        )?;
        update_witnesses::<OrchardDomain>(
            orchard_notes_to_mark_position,
            &mut txmds_writelock,
            &wc,
        )?;

        // Return a nothing-value
        Ok::<(), String>(())
    }
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn trial_decrypt_domain_specific_outputs<D>(
        transaction_metadata: &mut bool,
        compact_transaction: &CompactTx,
        transaction_num: usize,
        compact_block: &CompactBlock,
        ivk: D::IncomingViewingKey,
        height: BlockHeight,
        config: &crate::config::ZingoConfig,
        wc: &Arc<WalletCapability>,
        bsync_data: &Arc<RwLock<BlazeSyncData>>,
        transaction_metadata_set: &Arc<RwLock<TxMap>>,
        detected_transaction_id_sender: &UnboundedSender<(
            TxId,
            PoolNullifier,
            BlockHeight,
            Option<u32>,
            bool,
        )>,
        workers: &FuturesUnordered<JoinHandle<Result<(), String>>>,
        notes_to_mark_position: &mut [(
            u32,
            TxId,
            <D::WalletNote as ShieldedNoteInterface>::Node,
            Retention<BlockHeight>,
        )],
    ) where
        D: DomainWalletExt,
        <D as Domain>::Recipient: crate::wallet::traits::Recipient + Send + 'static,
        <D as Domain>::Note: PartialEq + Send + 'static + Clone,
        <D as Domain>::ExtractedCommitmentBytes: Into<[u8; 32]>,
        <<D as DomainWalletExt>::WalletNote as ShieldedNoteInterface>::Node: PartialEq,
    {
        let transaction_id = txid_from_slice(&compact_transaction.hash);
        let outputs = D::CompactOutput::from_compact_transaction(compact_transaction)
            .iter()
            .map(|output| {
                (
                    output.domain(config.chain, height),
                    output.to_compact_output_impl(),
                )
            })
            .collect::<Vec<_>>();
        let maybe_decrypted_outputs =
            zcash_note_encryption::batch::try_compact_note_decryption(&[ivk], &outputs);

        for maybe_decrypted_output in maybe_decrypted_outputs.into_iter().enumerate() {
            let (output_num, witnessed) =
                if let (i, Some(((note, to), _ivk_num))) = maybe_decrypted_output {
                    *transaction_metadata = true; // i.e. we got metadata

                    let wc = wc.clone();
                    let bsync_data = bsync_data.clone();
                    let transaction_metadata_set = transaction_metadata_set.clone();
                    let detected_transaction_id_sender = detected_transaction_id_sender.clone();
                    let timestamp = compact_block.time;
                    let config = config.clone();

                    workers.push(tokio::spawn(async move {
                        let Ok(fvk) = D::unified_key_store_to_fvk(wc.unified_key_store()) else {
                            // skip any scanning if the wallet doesn't have viewing capability
                            return Ok::<_, String>(());
                        };

                        //TODO: Wrong. We don't have fvk import, all our keys are spending
                        let have_spending_key = true;

                        // Get the witness for the note
                        let witness = bsync_data
                            .read()
                            .await
                            .block_data
                            .get_note_witness::<D>(
                                config.get_lightwalletd_uri(),
                                height,
                                transaction_num,
                                i,
                                config.chain.activation_height(D::NU).unwrap().into(),
                            )
                            .await?;

                        let spend_nullifier = D::get_nullifier_from_note_fvk_and_witness_position(
                            &note,
                            &fvk,
                            u64::from(witness.witnessed_position()),
                        );

                        let status = ConfirmationStatus::Confirmed(height);
                        transaction_metadata_set
                            .write()
                            .await
                            .transaction_records_by_id
                            .add_new_note::<D>(
                                transaction_id,
                                status,
                                Some(timestamp),
                                note,
                                to,
                                have_spending_key,
                                Some(spend_nullifier),
                                i as u32,
                                witness.witnessed_position(),
                            );

                        debug!("Trial decrypt Detected txid {}", &transaction_id);

                        detected_transaction_id_sender
                            .send((
                                transaction_id,
                                spend_nullifier.into(),
                                height,
                                Some(i as u32),
                                true,
                            ))
                            .unwrap();

                        Ok::<_, String>(())
                    }));
                    (i, true)
                } else {
                    (maybe_decrypted_output.0, false)
                };
            if witnessed {
                notes_to_mark_position[output_num].3 = Retention::Marked
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn zip_outputs_with_retention_txids_indexes<D: DomainWalletExt>(
    compact_transaction: &CompactTx,
) -> Vec<(
    u32,
    TxId,
    <D::WalletNote as ShieldedNoteInterface>::Node,
    Retention<BlockHeight>,
)>
where
    <D as Domain>::Note: PartialEq + Clone,
    <D as Domain>::Recipient: crate::wallet::traits::Recipient,
{
    <D::CompactOutput as crate::wallet::traits::CompactOutput<D>>::from_compact_transaction(
        compact_transaction,
    )
    .iter()
    .enumerate()
    .map(|(i, output)| {
        (
            i as u32,
            TxId::from_bytes(<[u8; 32]>::try_from(&compact_transaction.hash[..]).unwrap()),
            <D::WalletNote as ShieldedNoteInterface>::Node::from_commitment(output.cmstar())
                .unwrap(),
            Retention::Ephemeral,
        )
    })
    .collect()
}

#[allow(clippy::type_complexity)]
fn update_witnesses<D>(
    notes_to_mark_position: Vec<(
        Vec<(
            u32,
            TxId,
            <D::WalletNote as ShieldedNoteInterface>::Node,
            Retention<BlockHeight>,
        )>,
        BlockHeight,
    )>,
    txmds_writelock: &mut TxMap,
    wc: &Arc<WalletCapability>,
) -> ZingoLibResult<()>
where
    D: DomainWalletExt,
    <D as Domain>::Note: PartialEq + Clone,
    <D as Domain>::Recipient: Recipient,
{
    for block in notes_to_mark_position.into_iter().rev() {
        if let Some(witness_tree) = D::transaction_metadata_set_to_shardtree(&*txmds_writelock) {
            let position = witness_tree
                .max_leaf_position(None)
                .unwrap()
                .map(|pos| pos + 1)
                .unwrap_or(Position::from(0));
            let mut nodes_retention = Vec::new();
            for (i, (output_index, transaction_id, node, retention)) in
                block.0.into_iter().enumerate()
            {
                if retention != Retention::Ephemeral {
                    txmds_writelock.mark_note_position::<D>(
                        transaction_id,
                        Some(output_index),
                        position + i as u64,
                        &D::unified_key_store_to_fvk(wc.unified_key_store()).unwrap(),
                    )?;
                }
                nodes_retention.push((node, retention));
            }
            if let Some(witness_tree_mut) =
                D::transaction_metadata_set_to_shardtree_mut(&mut *txmds_writelock)
            {
                let _tree_insert_result = witness_tree_mut
                    .batch_insert(position, nodes_retention.into_iter())
                    .expect("failed to update witness tree");
                witness_tree_mut.checkpoint(block.1).expect("Infallible");
            }
        }
    }
    Ok(())
}

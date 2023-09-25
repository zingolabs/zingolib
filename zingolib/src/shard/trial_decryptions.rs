//! The zcash blockchain and proxied transformations derived from it, do not contain any public
//! information about transaction recipients. Therefore clients must attempt to decrypt each
//! note with each of their keys to determine if they are the recipient.
//! This process is called: `trial_decryption`.

use crate::{
    compact_formats::{CompactBlock, CompactTx},
    wallet::{
        data::{PoolNullifier, TransactionMetadata},
        keys::unified::WalletCapability,
        traits::{
            CompactOutput as _, DomainWalletExt, FromCommitment, ReceivedNoteAndMetadata, Recipient,
        },
        transactions::TransactionMetadataSet,
        MemoDownloadOption,
    },
};
use futures::{stream::FuturesUnordered, StreamExt};
use incrementalmerkletree::{Position, Retention};
use log::debug;
use orchard::{keys::IncomingViewingKey as OrchardIvk, note_encryption::OrchardDomain};
use std::sync::Arc;
use tokio::{
    sync::{
        mpsc::{unbounded_channel, UnboundedSender},
        oneshot, RwLock,
    },
    task::JoinHandle,
};
use zcash_note_encryption::Domain;
use zcash_primitives::{
    consensus::{BlockHeight, Parameters},
    sapling::{note_encryption::SaplingDomain, SaplingIvk},
    transaction::{Transaction, TxId},
};
use zingoconfig::{ChainType, ZingoConfig};

use super::syncdata::ShardSyncData;

pub struct TrialDecryptions {
    wc: Arc<WalletCapability>,
    transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
    config: Arc<ZingoConfig>,
}

impl TrialDecryptions {
    pub fn new(
        config: Arc<ZingoConfig>,
        wc: Arc<WalletCapability>,
        transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
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
        sync_data: Arc<RwLock<ShardSyncData>>,
        detected_transaction_id_sender: UnboundedSender<(
            TxId,
            PoolNullifier,
            BlockHeight,
            Option<u32>,
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

            let sapling_ivk = SaplingIvk::try_from(&*wc).ok();
            let orchard_ivk = orchard::keys::IncomingViewingKey::try_from(&*wc).ok();

            while let Some(cb) = receiver.recv().await {
                cbs.push(cb);

                if cbs.len() >= 125 {
                    // We seem to have enough to delegate to a new thread.
                    // Why 1000?
                    let wc = wc.clone();
                    let sapling_ivk = sapling_ivk.clone();
                    let orchard_ivk = orchard_ivk.clone();
                    let transaction_metadata_set = transaction_metadata_set.clone();
                    let sync_data = sync_data.clone();
                    let detected_transaction_id_sender = detected_transaction_id_sender.clone();
                    let config = config.clone();

                    workers.push(tokio::spawn(Self::trial_decrypt_batch(
                        config,
                        cbs.split_off(0), // This allocates all received cbs to the spawn.
                        wc,
                        sync_data,
                        sapling_ivk,
                        orchard_ivk,
                        transaction_metadata_set,
                        transaction_size_filter,
                        detected_transaction_id_sender,
                        full_transaction_fetcher.clone(),
                    )));
                }
            }
            // Finish off the remaining < 1000 cbs
            workers.push(tokio::spawn(Self::trial_decrypt_batch(
                config,
                cbs,
                wc,
                sync_data,
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
        sync_data: Arc<RwLock<ShardSyncData>>,
        sapling_ivk: Option<SaplingIvk>,
        orchard_ivk: Option<OrchardIvk>,
        transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
        transaction_size_filter: Option<u32>,
        detected_transaction_id_sender: UnboundedSender<(
            TxId,
            PoolNullifier,
            BlockHeight,
            Option<u32>,
        )>,
        full_transaction_fetcher: UnboundedSender<(
            TxId,
            oneshot::Sender<Result<Transaction, String>>,
        )>,
    ) -> Result<(), String> {
        let mut workers = FuturesUnordered::new();

        let download_memos = sync_data.read().await.wallet_options.download_memos;
        let mut sapling_notes_to_mark_position = Vec::new();
        let mut orchard_notes_to_mark_position = Vec::new();

        for compact_block in compact_blocks {
            let height = BlockHeight::from_u32(compact_block.height as u32);
            let mut sapling_notes_to_mark_position_in_block = Vec::new();
            let mut orchard_notes_to_mark_position_in_block = Vec::new();

            for (transaction_num, compact_transaction) in compact_block.vtx.iter().enumerate() {
                if let Some(filter) = transaction_size_filter {
                    if compact_transaction.outputs.len() + compact_transaction.actions.len()
                        > filter as usize
                    {
                        break;
                    }
                }
                let mut transaction_metadata = false;

                if let Some(sapling_notes_to_mark_position_in_tx) =
                    sapling_ivk.as_ref().map(|sapling_ivk| {
                        Self::trial_decrypt_domain_specific_outputs::<
                        SaplingDomain<zingoconfig::ChainType>,
                    >(
                        &mut transaction_metadata,
                        compact_transaction,
                        transaction_num,
                        &compact_block,
                        zcash_primitives::sapling::note_encryption::PreparedIncomingViewingKey::new(
                            sapling_ivk,
                        ),
                        height,
                        &config,
                        &wc,
                        &sync_data,
                        &transaction_metadata_set,
                        &detected_transaction_id_sender,
                        &workers,
                    )
                    })
                {
                    sapling_notes_to_mark_position_in_block
                        .extend_from_slice(&sapling_notes_to_mark_position_in_tx)
                };

                if let Some(orchard_notes_to_mark_position_in_tx) =
                    orchard_ivk.as_ref().map(|orchard_ivk| {
                        Self::trial_decrypt_domain_specific_outputs::<OrchardDomain>(
                            &mut transaction_metadata,
                            compact_transaction,
                            transaction_num,
                            &compact_block,
                            orchard::keys::PreparedIncomingViewingKey::new(orchard_ivk),
                            height,
                            &config,
                            &wc,
                            &sync_data,
                            &transaction_metadata_set,
                            &detected_transaction_id_sender,
                            &workers,
                        )
                    })
                {
                    orchard_notes_to_mark_position_in_block
                        .extend_from_slice(&orchard_notes_to_mark_position_in_tx)
                };

                // Check option to see if we are fetching all transactions.
                // grabs all memos regardless of decryption status.
                // Note the memos are immediately discarded.
                // Perhaps this obfuscates the memos of interest?
                if !transaction_metadata && download_memos == MemoDownloadOption::AllMemos {
                    let transaction_id = TransactionMetadata::new_txid(&compact_transaction.hash);
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
        update_witnesses::<SaplingDomain<ChainType>>(
            sapling_notes_to_mark_position,
            &mut txmds_writelock,
            &wc,
        );
        update_witnesses::<OrchardDomain>(
            orchard_notes_to_mark_position,
            &mut txmds_writelock,
            &wc,
        );

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
        config: &zingoconfig::ZingoConfig,
        wc: &Arc<WalletCapability>,
        sync_data: &Arc<RwLock<ShardSyncData>>,
        transaction_metadata_set: &Arc<RwLock<TransactionMetadataSet>>,
        detected_transaction_id_sender: &UnboundedSender<(
            TxId,
            PoolNullifier,
            BlockHeight,
            Option<u32>,
        )>,
        workers: &FuturesUnordered<JoinHandle<Result<(), String>>>,
    ) -> Vec<(
        usize,
        TxId,
        (
            <D::WalletNote as ReceivedNoteAndMetadata>::Node,
            Retention<BlockHeight>,
        ),
    )>
    where
        D: DomainWalletExt,
        <D as Domain>::Recipient: crate::wallet::traits::Recipient + Send + 'static,
        <D as Domain>::Note: PartialEq + Send + 'static + Clone,
        <D as Domain>::ExtractedCommitmentBytes: Into<[u8; 32]>,
        <<D as DomainWalletExt>::WalletNote as ReceivedNoteAndMetadata>::Node: PartialEq,
    {
        let mut witness_txindexes_notes_commitments = Vec::new();
        let transaction_id = TransactionMetadata::new_txid(&compact_transaction.hash);
        let outputs = D::CompactOutput::from_compact_transaction(compact_transaction)
            .iter()
            .map(|output| (output.domain(config.chain, height), output.clone()))
            .collect::<Vec<_>>();
        let maybe_decrypted_outputs =
            zcash_note_encryption::batch::try_compact_note_decryption(&[ivk], &outputs);
        for maybe_decrypted_output in maybe_decrypted_outputs.into_iter().enumerate() {
            let (output_num, witnessed) =
                if let (i, Some(((note, to), _ivk_num))) = maybe_decrypted_output {
                    *transaction_metadata = true; // i.e. we got metadata

                    let wc = wc.clone();
                    let sync_data = sync_data.clone();
                    let transaction_metadata_set = transaction_metadata_set.clone();
                    let detected_transaction_id_sender = detected_transaction_id_sender.clone();
                    let timestamp = compact_block.time as u64;
                    let config = config.clone();

                    workers.push(tokio::spawn(async move {
                        let Ok(fvk) = D::wc_to_fvk(&wc) else {
                            // skip any scanning if the wallet doesn't have viewing capability
                            return Ok::<_, String>(());
                        };

                        //TODO: Wrong. We don't have fvk import, all our keys are spending
                        let have_spending_key = true;
                        let uri = sync_data.read().await.uri().clone();

                        // Get the witness for the note
                        let witness = sync_data
                            .read()
                            .await
                            .block_data
                            .get_note_witness::<D>(
                                uri,
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

                        transaction_metadata_set.write().await.add_new_note::<D>(
                            transaction_id,
                            height,
                            false,
                            timestamp,
                            note,
                            to,
                            have_spending_key,
                            Some(spend_nullifier),
                            i,
                        );

                        debug!("Trial decrypt Detected txid {}", &transaction_id);

                        detected_transaction_id_sender
                            .send((
                                transaction_id,
                                spend_nullifier.into(),
                                height,
                                Some((i) as u32),
                            ))
                            .unwrap();

                        Ok::<_, String>(())
                    }));
                    (i, true)
                } else {
                    (maybe_decrypted_output.0, false)
                };
            witness_txindexes_notes_commitments.push((output_num, transaction_id, (

                <<D::WalletNote as ReceivedNoteAndMetadata>::Node as FromCommitment>::from_commitment(
                    outputs[output_num].1.cmstar(),
                ).unwrap(),
                match witnessed {
                    true => Retention::Marked,
                    false => Retention::Ephemeral,
                }
                    )
                ));
        }
        witness_txindexes_notes_commitments
    }
}

#[allow(clippy::type_complexity)]
fn update_witnesses<D>(
    notes_to_mark_position: Vec<(
        Vec<(
            usize,
            TxId,
            (
                <D::WalletNote as ReceivedNoteAndMetadata>::Node,
                Retention<BlockHeight>,
            ),
        )>,
        BlockHeight,
    )>,
    txmds_writelock: &mut TransactionMetadataSet,
    wc: &Arc<WalletCapability>,
) where
    D: DomainWalletExt,
    <D as Domain>::Note: PartialEq + Clone,
    <D as Domain>::Recipient: Recipient,
{
    for block in notes_to_mark_position.into_iter().rev() {
        if let Some(witness_tree) = D::get_shardtree(&*txmds_writelock) {
            let position = witness_tree
                .max_leaf_position(0)
                .unwrap()
                .map(|pos| pos + 1)
                .unwrap_or(Position::from(0));
            let mut nodes_retention = Vec::new();
            for (i, (output_num, transaction_id, (node, retention))) in
                block.0.into_iter().enumerate()
            {
                if retention != Retention::Ephemeral {
                    txmds_writelock.mark_note_position::<D>(
                        transaction_id,
                        output_num,
                        position + i as u64,
                        &D::wc_to_fvk(wc).unwrap(),
                    );
                }
                nodes_retention.push((node, retention));
            }
            if let Some(witness_tree_mut) = D::get_shardtree_mut(&mut *txmds_writelock) {
                let _tree_insert_result = witness_tree_mut
                    .batch_insert(position, nodes_retention.into_iter())
                    .expect("failed to update witness tree");
                witness_tree_mut.checkpoint(block.1).expect("Infallible");
            }
        }
    }
}

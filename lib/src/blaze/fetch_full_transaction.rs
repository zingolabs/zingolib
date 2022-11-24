use crate::wallet::{
    data::OutgoingTxMetadata,
    keys::{address_from_pubkeyhash, unified::UnifiedSpendCapability, ToBase58Check},
    traits::{
        self as zingo_traits, Bundle as _, DomainWalletExt, Nullifier as _,
        ReceivedNoteAndMetadata as _, Recipient as _, ShieldedOutputExt as _, Spend as _,
        ToBytes as _,
    },
    transactions::TransactionMetadataSet,
};

use futures::{future::join_all, stream::FuturesUnordered, StreamExt};
use log::info;
use orchard::note_encryption::OrchardDomain;
use std::{
    collections::HashSet,
    convert::TryInto,
    io::Read,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::{
    sync::{
        mpsc::{unbounded_channel, UnboundedSender},
        oneshot, RwLock,
    },
    task::JoinHandle,
};
use zcash_client_backend::address::{RecipientAddress, UnifiedAddress};
use zcash_note_encryption::try_output_recovery_with_ovk;

use zcash_primitives::{
    consensus::BlockHeight,
    legacy::TransparentAddress,
    memo::{Memo, MemoBytes},
    sapling::note_encryption::SaplingDomain,
    transaction::{Transaction, TxId},
};

use super::syncdata::BlazeSyncData;
use zingoconfig::{ChainType, ZingoConfig};

#[derive(Clone)]
pub struct TransactionContext {
    pub(crate) config: ZingoConfig,
    pub(crate) key: Arc<RwLock<UnifiedSpendCapability>>,
    pub(crate) transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
}

impl TransactionContext {
    pub fn new(
        config: &ZingoConfig,
        key: Arc<RwLock<UnifiedSpendCapability>>,
        transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
    ) -> Self {
        Self {
            config: config.clone(),
            key,
            transaction_metadata_set,
        }
    }

    pub(crate) async fn scan_full_tx(
        &self,
        transaction: Transaction,
        height: BlockHeight,
        unconfirmed: bool,
        block_time: u32,
        price: Option<f64>,
    ) {
        // Remember if this is an outgoing Tx. Useful for when we want to grab the outgoing metadata.
        let mut is_outgoing_transaction = false;

        // Collect our t-addresses for easy checking
        let taddrs_set = self.key.read().await.get_all_taddrs(&self.config);

        //todo: investigate scanning all bundles simultaneously
        self.scan_transparent_bundle(
            &transaction,
            height,
            unconfirmed,
            block_time,
            &mut is_outgoing_transaction,
            &taddrs_set,
        )
        .await;

        let mut outgoing_metadatas = vec![];
        self.scan_sapling_bundle(
            &transaction,
            height,
            unconfirmed,
            block_time,
            &mut is_outgoing_transaction,
            &mut outgoing_metadatas,
        )
        .await;
        self.scan_orchard_bundle(
            &transaction,
            height,
            unconfirmed,
            block_time,
            &mut is_outgoing_transaction,
            &mut outgoing_metadatas,
        )
        .await;

        // Process t-address outputs
        // If this transaction in outgoing, i.e., we recieved sent some money in this transaction, then we need to grab all transparent outputs
        // that don't belong to us as the outgoing metadata
        if self
            .transaction_metadata_set
            .read()
            .await
            .total_funds_spent_in(&transaction.txid())
            > 0
        {
            is_outgoing_transaction = true;
        }

        if is_outgoing_transaction {
            if let Some(t_bundle) = transaction.transparent_bundle() {
                for vout in &t_bundle.vout {
                    let taddr = address_from_pubkeyhash(&self.config, vout.script_pubkey.address());

                    if taddr.is_some() && !taddrs_set.contains(taddr.as_ref().unwrap()) {
                        outgoing_metadatas.push(OutgoingTxMetadata {
                            address: taddr.unwrap(),
                            value: vout.value.into(),
                            memo: Memo::Empty,
                            ua: None,
                        });
                    }
                }
            }

            // Also, if this is an outgoing transaction, then mark all the *incoming* sapling notes to this transaction as change.
            // Note that this is also done in `WalletTxns::add_new_spent`, but that doesn't take into account transparent spends,
            // so we'll do it again here.
            self.transaction_metadata_set
                .write()
                .await
                .check_notes_mark_change(&transaction.txid());
        }

        if !outgoing_metadatas.is_empty() {
            self.transaction_metadata_set
                .write()
                .await
                .add_outgoing_metadata(&transaction.txid(), outgoing_metadatas);
        }

        // Update price if available
        if price.is_some() {
            self.transaction_metadata_set
                .write()
                .await
                .set_price(&transaction.txid(), price);
        }

        //info!("Finished Fetching full transaction {}", tx.txid());
    }

    async fn scan_transparent_bundle(
        &self,
        transaction: &Transaction,
        height: BlockHeight,
        unconfirmed: bool,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        taddrs_set: &HashSet<String>,
    ) {
        // Scan all transparent outputs to see if we recieved any money
        if let Some(t_bundle) = transaction.transparent_bundle() {
            for (n, vout) in t_bundle.vout.iter().enumerate() {
                match vout.script_pubkey.address() {
                    Some(TransparentAddress::PublicKey(hash)) => {
                        let output_taddr =
                            hash.to_base58check(&self.config.base58_pubkey_address(), &[]);
                        if taddrs_set.contains(&output_taddr) {
                            // This is our address. Add this as an output to the txid
                            self.transaction_metadata_set
                                .write()
                                .await
                                .add_new_taddr_output(
                                    transaction.txid(),
                                    output_taddr.clone(),
                                    height.into(),
                                    unconfirmed,
                                    block_time as u64,
                                    &vout,
                                    n as u32,
                                );

                            // Ensure that we add any new HD addresses
                            // TODO: I don't think we need to do this anymore
                            // self.keys.write().await.ensure_hd_taddresses(&output_taddr);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Scan transparent spends

        // Scan all the inputs to see if we spent any transparent funds in this tx
        let mut total_transparent_value_spent = 0;
        let mut spent_utxos = vec![];

        {
            let current = &self.transaction_metadata_set.read().await.current;
            if let Some(t_bundle) = transaction.transparent_bundle() {
                for vin in t_bundle.vin.iter() {
                    // Find the prev txid that was spent
                    let prev_transaction_id = TxId::from_bytes(*vin.prevout.hash());
                    let prev_n = vin.prevout.n() as u64;

                    if let Some(wtx) = current.get(&prev_transaction_id) {
                        // One of the tx outputs is a match
                        if let Some(spent_utxo) = wtx
                            .utxos
                            .iter()
                            .find(|u| u.txid == prev_transaction_id && u.output_index == prev_n)
                        {
                            info!(
                                "Spent: utxo from {} was spent in {}",
                                prev_transaction_id,
                                transaction.txid()
                            );
                            total_transparent_value_spent += spent_utxo.value;
                            spent_utxos.push((
                                prev_transaction_id,
                                prev_n as u32,
                                transaction.txid(),
                                height,
                            ));
                        }
                    }
                }
            }
        }

        // Mark all the UTXOs that were spent here back in their original txns.
        for (prev_transaction_id, prev_n, transaction_id, height) in spent_utxos {
            // Mark that this Tx spent some funds
            *is_outgoing_transaction = true;

            self.transaction_metadata_set
                .write()
                .await
                .mark_txid_utxo_spent(prev_transaction_id, prev_n, transaction_id, height.into());
        }

        // If this transaction spent value, add the spent amount to the TxID
        if total_transparent_value_spent > 0 {
            *is_outgoing_transaction = true;

            self.transaction_metadata_set.write().await.add_taddr_spent(
                transaction.txid(),
                height,
                unconfirmed,
                block_time as u64,
                total_transparent_value_spent,
            );
        }
    }
    async fn scan_sapling_bundle(
        &self,
        transaction: &Transaction,
        height: BlockHeight,
        pending: bool,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        outgoing_metadatas: &mut Vec<OutgoingTxMetadata>,
    ) {
        self.scan_bundle::<SaplingDomain<ChainType>>(
            transaction,
            height,
            pending,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
        )
        .await
    }
    async fn scan_orchard_bundle(
        &self,
        transaction: &Transaction,
        height: BlockHeight,
        pending: bool,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        outgoing_metadatas: &mut Vec<OutgoingTxMetadata>,
    ) {
        self.scan_bundle::<OrchardDomain>(
            transaction,
            height,
            pending,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
        )
        .await;
    }

    /// Transactions contain per-protocol "bundles" of components.
    /// The component details vary by protocol.
    /// In Sapling the components are "Spends" and "Outputs"
    /// In Orchard the components are "Actions", each of which
    /// _IS_ 1 Spend and 1 Output.
    async fn scan_bundle<D>(
        &self,
        transaction: &Transaction,
        transaction_block_height: BlockHeight, // TODO: Note that this parameter is NA in the case of "unconfirmed"
        pending: bool, // TODO: This is true when called by wallet.send_to_address_internal, investigate.
        block_time: u32,
        is_outgoing_transaction: &mut bool, // Isn't this also NA for unconfirmed?
        outgoing_metadatas: &mut Vec<OutgoingTxMetadata>,
    ) where
        D: zingo_traits::DomainWalletExt<ChainType>,
        D::Note: Clone + PartialEq,
        D::OutgoingViewingKey: std::fmt::Debug,
        D::Recipient: zingo_traits::Recipient,
        D::Memo: zingo_traits::ToBytes<512>,
    {
        type FnGenBundle<I> = <I as DomainWalletExt<ChainType>>::Bundle;
        // Check if any of the nullifiers generated in this transaction are ours. We only need this for unconfirmed transactions,
        // because for transactions in the block, we will check the nullifiers from the blockdata
        if pending {
            let unspent_nullifiers =
            <<D as DomainWalletExt<ChainType>>
              ::WalletNote as zingo_traits::ReceivedNoteAndMetadata>
                ::Nullifier::get_nullifiers_of_unspent_notes_from_transaction_set(
                &*self.transaction_metadata_set.read().await,
            );
            for output in <FnGenBundle<D> as zingo_traits::Bundle<D, ChainType>>::from_transaction(
                transaction,
            )
            .into_iter()
            .flat_map(|bundle| bundle.spend_elements().into_iter())
            {
                if let Some((nf, value, transaction_id)) = unspent_nullifiers
                    .iter()
                    .find(|(nf, _, _)| nf == output.nullifier())
                {
                    self.transaction_metadata_set.write().await.add_new_spent(
                    transaction.txid(),
                    transaction_block_height,
                    true, // this was "unconfirmed" but this fn is invoked inside `if unconfirmed` TODO: add regression test to protect against movement
                    block_time,
                    <FnGenBundle<D> as zingo_traits::Bundle<D, ChainType>>::Spend::wallet_nullifier(
                        nf,
                    ),
                    *value,
                    *transaction_id,
                );
                }
            }
        }
        // The preceding updates the wallet_transactions with presumptive new "spent" nullifiers.  I continue to find the notion
        // of a "spent" nullifier to be problematic.
        // Issues:
        //     1. There's more than one way to be "spent".
        //     2. It's possible for a "nullifier" to be in the wallet's spent list, but never in the global ledger.
        //     <https://github.com/zingolabs/zingolib/issues/65>
        let unified_spend_capability = self.key.read().await;
        let domain_tagged_outputs =
            <FnGenBundle<D> as zingo_traits::Bundle<D, ChainType>>::from_transaction(transaction)
                .into_iter()
                .flat_map(|bundle| bundle.output_elements().into_iter())
                .map(|output| {
                    (
                        output.domain(transaction_block_height, self.config.chain),
                        output.clone(),
                    )
                })
                .collect::<Vec<_>>();

        let ivk = D::usc_to_ivk(&*unified_spend_capability);
        let mut decrypt_attempts =
            zcash_note_encryption::batch::try_note_decryption(&[ivk], &domain_tagged_outputs)
                .into_iter();
        while let Some(decrypt_attempt) = decrypt_attempts.next() {
            let ((note, to, memo_bytes), _ivk_num) = match decrypt_attempt {
                Some(plaintext) => plaintext,
                _ => continue,
            };
            let memo_bytes = MemoBytes::from_bytes(&memo_bytes.to_bytes()).unwrap();
            if pending {
                self.transaction_metadata_set
                    .write()
                    .await
                    .add_pending_note::<D>(
                        transaction.txid(),
                        transaction_block_height,
                        block_time as u64,
                        note.clone(),
                        to,
                        &D::usc_to_fvk(&*unified_spend_capability),
                    );
            }
            let memo = memo_bytes
                .clone()
                .try_into()
                .unwrap_or(Memo::Future(memo_bytes));
            if let Memo::Arbitrary(ref wallet_internal_data) = memo {
                let mut uas_bytes = vec![0; 511];
                wallet_internal_data
                    .as_slice()
                    .read(&mut uas_bytes)
                    .unwrap();
                let uas_string = String::from_utf8(uas_bytes);
                if let Ok(uas_string) = uas_string {
                    for ua_string in uas_string.split('!') {
                        if let Some(RecipientAddress::Unified(ua)) =
                            RecipientAddress::decode(&self.config.chain, ua_string)
                        {
                            if let Some(transaction) = self
                                .transaction_metadata_set
                                .write()
                                .await
                                .current
                                .get_mut(&transaction.txid())
                            {
                                let outgoing_potential_recievers = [
                                    ua.orchard().map(|oaddr| {
                                        oaddr.b32encode_for_network(&self.config.chain)
                                    }),
                                    ua.sapling().map(|zaddr| {
                                        zaddr.b32encode_for_network(&self.config.chain)
                                    }),
                                    address_from_pubkeyhash(
                                        &self.config,
                                        ua.transparent().cloned(),
                                    ),
                                ];
                                if let Some(metadata) =
                                    transaction.outgoing_metadata.iter_mut().find(|metadata| {
                                        outgoing_potential_recievers
                                            .contains(&Some(metadata.address.clone()))
                                    })
                                {
                                    metadata.ua = Some(ua_string.to_string());
                                }
                            }
                        }
                    }
                }
            }
            self.transaction_metadata_set
                .write()
                .await
                .add_memo_to_note_metadata::<D::WalletNote>(&transaction.txid(), note, memo);
        }
        for (_domain, output) in domain_tagged_outputs {
            outgoing_metadatas.extend(
                match try_output_recovery_with_ovk::<
                    D,
                    <FnGenBundle<D> as zingo_traits::Bundle<D, ChainType>>::Output,
                >(
                    &output.domain(transaction_block_height, self.config.chain),
                    &D::usc_to_ovk(&*unified_spend_capability),
                    &output,
                    &output.value_commitment(),
                    &output.out_ciphertext(),
                ) {
                    Some((note, payment_address, memo_bytes)) => {
                        // Mark this transaction as an outgoing transaction, so we can grab all outgoing metadata
                        *is_outgoing_transaction = true;
                        let address = payment_address.b32encode_for_network(&self.config.chain);

                        // Check if this is change, and if it also doesn't have a memo, don't add
                        // to the outgoing metadata.
                        // If this is change (i.e., funds sent to ourself) AND has a memo, then
                        // presumably the user is writing a memo to themself, so we will add it to
                        // the outgoing metadata, even though it might be confusing in the UI, but hopefully
                        // the user can make sense of it.
                        match Memo::from_bytes(&memo_bytes.to_bytes()) {
                            Err(_) => None,
                            Ok(memo) => {
                                if unified_spend_capability.addresses().iter().any(
                                    |unified_address| {
                                        [
                                            unified_address
                                                .transparent()
                                                .cloned()
                                                .map(RecipientAddress::from),
                                            unified_address
                                                .sapling()
                                                .cloned()
                                                .map(RecipientAddress::from),
                                            unified_address.orchard().cloned().map(
                                                |orchard_receiver| {
                                                    RecipientAddress::from(
                                                        UnifiedAddress::from_receivers(
                                                            Some(orchard_receiver),
                                                            None,
                                                            None,
                                                        )
                                                        .unwrap(),
                                                    )
                                                },
                                            ),
                                        ]
                                        .into_iter()
                                        .filter_map(std::convert::identity)
                                        .map(|addr| addr.encode(&self.config.chain))
                                        .any(|addr| addr == address)
                                    },
                                ) {
                                    if let Memo::Text(_) = memo {
                                        Some(OutgoingTxMetadata {
                                            address,
                                            value: D::WalletNote::value_from_note(&note),
                                            memo,
                                            ua: None,
                                        })
                                    } else {
                                        None
                                    }
                                } else {
                                    Some(OutgoingTxMetadata {
                                        address,
                                        value: D::WalletNote::value_from_note(&note),
                                        memo,
                                        ua: None,
                                    })
                                }
                            }
                        }
                    }
                    None => None,
                },
            );
        }
    }
}

pub async fn start(
    transaction_context: TransactionContext,
    fulltx_fetcher: UnboundedSender<(TxId, oneshot::Sender<Result<Transaction, String>>)>,
    bsync_data: Arc<RwLock<BlazeSyncData>>,
) -> (
    JoinHandle<Result<(), String>>,
    UnboundedSender<(TxId, BlockHeight)>,
    UnboundedSender<(Transaction, BlockHeight)>,
) {
    let local_transaction_context = transaction_context.clone();

    let start_height = bsync_data.read().await.sync_status.read().await.start_block;
    let end_height = bsync_data.read().await.sync_status.read().await.end_block;

    let bsync_data_i = bsync_data.clone();

    let (transaction_id_transmitter, mut transaction_id_receiver) =
        unbounded_channel::<(TxId, BlockHeight)>();
    let h1: JoinHandle<Result<(), String>> = tokio::spawn(async move {
        let last_progress = Arc::new(AtomicU64::new(0));
        let mut workers = FuturesUnordered::new();

        while let Some((transaction_id, height)) = transaction_id_receiver.recv().await {
            let per_txid_iter_context = local_transaction_context.clone();
            let block_time = bsync_data_i
                .read()
                .await
                .block_data
                .get_block_timestamp(&height)
                .await;
            let full_transaction_fetcher = fulltx_fetcher.clone();
            let bsync_data = bsync_data_i.clone();
            let last_progress = last_progress.clone();

            workers.push(tokio::spawn(async move {
                // It is possible that we recieve the same txid multiple times, so we keep track of all the txids that were fetched
                let transaction = {
                    // Fetch the TxId from LightwalletD and process all the parts of it.
                    let (transmitter, receiver) = oneshot::channel();
                    full_transaction_fetcher
                        .send((transaction_id, transmitter))
                        .unwrap();
                    receiver.await.unwrap()?
                };

                let progress = start_height - u64::from(height);
                if progress > last_progress.load(Ordering::SeqCst) {
                    bsync_data
                        .read()
                        .await
                        .sync_status
                        .write()
                        .await
                        .txn_scan_done = progress;
                    last_progress.store(progress, Ordering::SeqCst);
                }

                per_txid_iter_context
                    .scan_full_tx(transaction, height, false, block_time, None)
                    .await;

                Ok::<_, String>(())
            }));
        }

        while let Some(r) = workers.next().await {
            r.map_err(|r| r.to_string())??;
        }

        bsync_data_i
            .read()
            .await
            .sync_status
            .write()
            .await
            .txn_scan_done = start_height - end_height + 1;
        //info!("Finished fetching all full transactions");

        Ok(())
    });

    let transaction_context = transaction_context.clone(); // TODO: Delete and study error.
    let (transaction_transmitter, mut transaction_receiver) =
        unbounded_channel::<(Transaction, BlockHeight)>();

    let h2: JoinHandle<Result<(), String>> = tokio::spawn(async move {
        let bsync_data = bsync_data.clone();

        while let Some((transaction, height)) = transaction_receiver.recv().await {
            let block_time = bsync_data
                .read()
                .await
                .block_data
                .get_block_timestamp(&height)
                .await;
            transaction_context
                .scan_full_tx(transaction, height, false, block_time, None)
                .await;
        }

        //info!("Finished full_tx scanning all txns");
        Ok(())
    });

    let h = tokio::spawn(async move {
        join_all(vec![h1, h2])
            .await
            .into_iter()
            .map(|r| r.map_err(|e| format!("{}", e))?)
            .collect::<Result<(), String>>()
    });

    return (h, transaction_id_transmitter, transaction_transmitter);
}

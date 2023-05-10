use crate::wallet::{
    data::OutgoingTxData,
    keys::{address_from_pubkeyhash, unified::WalletCapability},
    traits::{
        self as zingo_traits, Bundle as _, DomainWalletExt, Nullifier as _,
        ReceivedNoteAndMetadata as _, Recipient as _, ShieldedOutputExt as _, Spend as _,
        ToBytes as _,
    },
    transactions::TransactionMetadataSet,
};
use zingo_memo::{parse_zingo_memo, ParsedMemo};

use futures::{future::join_all, stream::FuturesUnordered, StreamExt};
use log::info;
use orchard::note_encryption::OrchardDomain;
use std::{
    collections::HashSet,
    convert::TryInto,
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
    memo::{Memo, MemoBytes},
    sapling::note_encryption::SaplingDomain,
    transaction::{Transaction, TxId},
};

use super::syncdata::BlazeSyncData;
use zingoconfig::{ChainType, ZingoConfig};

#[derive(Clone)]
pub struct TransactionContext {
    pub(crate) config: ZingoConfig,
    pub(crate) key: Arc<RwLock<WalletCapability>>,
    pub transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
}

impl TransactionContext {
    pub fn new(
        config: &ZingoConfig,
        key: Arc<RwLock<WalletCapability>>,
        transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
    ) -> Self {
        Self {
            config: config.clone(),
            key,
            transaction_metadata_set,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_bundlescans_internal(
        &self,
        transaction: &Transaction,
        height: BlockHeight,
        unconfirmed: bool,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        outgoing_metadatas: &mut Vec<OutgoingTxData>,
        arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
        taddrs_set: &HashSet<String>,
    ) {
        //todo: investigate scanning all bundles simultaneously
        self.scan_transparent_bundle(
            transaction,
            height,
            unconfirmed,
            block_time,
            is_outgoing_transaction,
            taddrs_set,
        )
        .await;

        self.scan_sapling_bundle(
            transaction,
            height,
            unconfirmed,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
            arbitrary_memos_with_txids,
        )
        .await;
        self.scan_orchard_bundle(
            transaction,
            height,
            unconfirmed,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
            arbitrary_memos_with_txids,
        )
        .await;
    }
    pub(crate) async fn scan_full_tx(
        &self,
        transaction: Transaction,
        height: BlockHeight,
        unconfirmed: bool,
        block_time: u32,
        price: Option<f64>,
    ) {
        // Set up data structures to record scan results
        let mut txid_indexed_zingo_memos = Vec::new();
        // Remember if this is an outgoing Tx. Useful for when we want to grab the outgoing metadata.
        let mut is_outgoing_transaction = false;
        // Collect our t-addresses for easy checking
        let taddrs_set = self.key.read().await.get_all_taddrs(&self.config);
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
        let mut outgoing_metadatas = vec![];
        // Execute scanning operations
        self.execute_bundlescans_internal(
            &transaction,
            height,
            unconfirmed,
            block_time,
            &mut is_outgoing_transaction,
            &mut outgoing_metadatas,
            &mut txid_indexed_zingo_memos,
            &taddrs_set,
        )
        .await;
        // Post process scan results
        if is_outgoing_transaction {
            if let Some(t_bundle) = transaction.transparent_bundle() {
                for vout in &t_bundle.vout {
                    if let Some(taddr) = vout
                        .recipient_address()
                        .map(|raw_taddr| address_from_pubkeyhash(&self.config, raw_taddr))
                    {
                        outgoing_metadatas.push(OutgoingTxData {
                            to_address: taddr,
                            value: u64::from(vout.value),
                            memo: Memo::Empty,
                            recipient_ua: None,
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

        self.update_outgoing_txdatas_with_uas(txid_indexed_zingo_memos)
            .await;

        // Update price if available
        if price.is_some() {
            self.transaction_metadata_set
                .write()
                .await
                .set_price(&transaction.txid(), price);
        }

        //info!("Finished Fetching full transaction {}", tx.txid());
    }

    async fn update_outgoing_txdatas_with_uas(
        &self,
        txid_indexed_zingo_memos: Vec<(ParsedMemo, TxId)>,
    ) {
        for (parsed_zingo_memo, txid) in txid_indexed_zingo_memos {
            match parsed_zingo_memo {
                ParsedMemo::Version0 { uas } => {
                    for ua in uas {
                        if let Some(transaction) = self
                            .transaction_metadata_set
                            .write()
                            .await
                            .current
                            .get_mut(&txid)
                        {
                            if !transaction.outgoing_tx_data.is_empty() {
                                let outgoing_potential_receivers = [
                                    ua.orchard().map(|oaddr| {
                                        oaddr.b32encode_for_network(&self.config.chain)
                                    }),
                                    ua.sapling().map(|zaddr| {
                                        zaddr.b32encode_for_network(&self.config.chain)
                                    }),
                                    ua.transparent()
                                        .map(|taddr| address_from_pubkeyhash(&self.config, *taddr)),
                                    Some(ua.encode(&self.config.chain)),
                                ];
                                if let Some(out_metadata) =
                                    transaction.outgoing_tx_data.iter_mut().find(|out_meta| {
                                        outgoing_potential_receivers
                                            .contains(&Some(out_meta.to_address.clone()))
                                    })
                                {
                                    out_metadata.recipient_ua = Some(ua.encode(&self.config.chain));
                                } else {
                                    log::error!(
                                        "Received memo indicating you sent to \
                                        an address you don't have on record.\n({})\n\
                                        This may mean you are being sent malicious data.\n\
                                        Some information may not be displayed correctly",
                                        ua.encode(&self.config.chain)
                                    )
                                }
                            }
                        }
                    }
                }
                other_memo_version => {
                    log::error!(
                        "Wallet internal memo is from a future version of the protocol\n\
                        Please ensure that your software is up-to-date.\n\
                        Memo: {other_memo_version:?}"
                    )
                }
            }
        }
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
                if let Some(taddr) = vout.recipient_address() {
                    let output_taddr = address_from_pubkeyhash(&self.config, taddr);
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
                                vout,
                                n as u32,
                            );

                        // Ensure that we add any new HD addresses
                        // TODO: I don't think we need to do this anymore
                        // self.keys.write().await.ensure_hd_taddresses(&output_taddr);
                    }
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
                            .received_utxos
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
    #[allow(clippy::too_many_arguments)]
    async fn scan_sapling_bundle(
        &self,
        transaction: &Transaction,
        height: BlockHeight,
        pending: bool,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        outgoing_metadatas: &mut Vec<OutgoingTxData>,
        arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
    ) {
        self.scan_bundle::<SaplingDomain<ChainType>>(
            transaction,
            height,
            pending,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
            arbitrary_memos_with_txids,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn scan_orchard_bundle(
        &self,
        transaction: &Transaction,
        height: BlockHeight,
        pending: bool,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        outgoing_metadatas: &mut Vec<OutgoingTxData>,
        arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
    ) {
        self.scan_bundle::<OrchardDomain>(
            transaction,
            height,
            pending,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
            arbitrary_memos_with_txids,
        )
        .await;
    }

    /// Transactions contain per-protocol "bundles" of components.
    /// The component details vary by protocol.
    /// In Sapling the components are "Spends" and "Outputs"
    /// In Orchard the components are "Actions", each of which
    /// _IS_ 1 Spend and 1 Output.
    #[allow(clippy::too_many_arguments)]
    async fn scan_bundle<D>(
        &self,
        transaction: &Transaction,
        transaction_block_height: BlockHeight, // TODO: Note that this parameter is NA in the case of "unconfirmed"
        pending: bool, // TODO: This is true when called by wallet.send_to_address_internal, investigate.
        block_time: u32,
        is_outgoing_transaction: &mut bool, // Isn't this also NA for unconfirmed?
        outgoing_metadatas: &mut Vec<OutgoingTxData>,
        arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
    ) where
        D: zingo_traits::DomainWalletExt,
        D::Note: Clone + PartialEq,
        D::OutgoingViewingKey: std::fmt::Debug,
        D::Recipient: zingo_traits::Recipient,
        D::Memo: zingo_traits::ToBytes<512>,
    {
        type FnGenBundle<I> = <I as DomainWalletExt>::Bundle;
        // Check if any of the nullifiers generated in this transaction are ours. We only need this for unconfirmed transactions,
        // because for transactions in the block, we will check the nullifiers from the blockdata
        if pending {
            let unspent_nullifiers =
            <<D as DomainWalletExt>
              ::WalletNote as zingo_traits::ReceivedNoteAndMetadata>
                ::Nullifier::get_nullifiers_of_unspent_notes_from_transaction_set(
                &*self.transaction_metadata_set.read().await,
            );
            for output in <FnGenBundle<D> as zingo_traits::Bundle<D>>::from_transaction(transaction)
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
                        (*nf).into(),
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
            <FnGenBundle<D> as zingo_traits::Bundle<D>>::from_transaction(transaction)
                .into_iter()
                .flat_map(|bundle| bundle.output_elements().into_iter())
                .map(|output| {
                    (
                        output.domain(transaction_block_height, self.config.chain),
                        output.clone(),
                    )
                })
                .collect::<Vec<_>>();

        let (Ok(ivk), Ok(ovk)) = (
            D::wc_to_ivk(&unified_spend_capability),
            D::wc_to_ovk(&unified_spend_capability),
        ) else {
            // skip scanning if wallet has not viewing capability
            return;
        };

        let decrypt_attempts =
            zcash_note_encryption::batch::try_note_decryption(&[ivk], &domain_tagged_outputs)
                .into_iter();
        for decrypt_attempt in decrypt_attempts {
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
                    );
            }
            let memo = memo_bytes
                .clone()
                .try_into()
                .unwrap_or(Memo::Future(memo_bytes));
            if let Memo::Arbitrary(ref wallet_internal_data) = memo {
                match parse_zingo_memo(*wallet_internal_data.as_ref()) {
                    Ok(parsed_zingo_memo) => {
                        arbitrary_memos_with_txids.push((parsed_zingo_memo, transaction.txid()));
                    }

                    Err(e) => log::error!(
                        "Could not decode wallet internal memo: {e}.\n\
                    Have you recently used a more up-to-date version of\
                    this software?\nIf not, this may mean you are being sent\
                    malicious data.\nSome information may not display correctly"
                    ),
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
                    <FnGenBundle<D> as zingo_traits::Bundle<D>>::Output,
                >(
                    &output.domain(transaction_block_height, self.config.chain),
                    &ovk,
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
                                        .flatten()
                                        .map(|addr| addr.encode(&self.config.chain))
                                        .any(|addr| addr == address)
                                    },
                                ) {
                                    if let Memo::Text(_) = memo {
                                        Some(OutgoingTxData {
                                            to_address: address,
                                            value: D::WalletNote::value_from_note(&note),
                                            memo,
                                            recipient_ua: None,
                                        })
                                    } else {
                                        None
                                    }
                                } else {
                                    Some(OutgoingTxData {
                                        to_address: address,
                                        value: D::WalletNote::value_from_note(&note),
                                        memo,
                                        recipient_ua: None,
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
            .try_for_each(|r| r.map_err(|e| format!("{}", e))?)
    });

    (h, transaction_id_transmitter, transaction_transmitter)
}

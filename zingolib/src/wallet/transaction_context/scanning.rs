use crate::{
    error::{ZingoLibError, ZingoLibResult},
    wallet::{
        data::OutgoingTxData,
        keys::{address_from_pubkeyhash, unified::WalletCapability},
        notes::ShieldedNoteInterface,
        traits::{
            self as zingo_traits, Bundle, DomainWalletExt, Recipient as _, ShieldedOutputExt as _,
            Spend as _, ToBytes as _,
        },
        transactions::TxMapAndMaybeTrees,
    },
};
use orchard::{keys::IncomingViewingKey, note_encryption::OrchardDomain};
use sapling_crypto::note_encryption::SaplingDomain;
use std::{collections::HashSet, convert::TryInto, sync::Arc};
use tokio::sync::RwLock;
use zcash_client_backend::address::{Address, UnifiedAddress};
use zcash_note_encryption::{try_output_recovery_with_ovk, Domain};
use zcash_primitives::{
    memo::{Memo, MemoBytes},
    transaction::{Transaction, TxId},
};
use zingo_memo::{parse_zingo_memo, ParsedMemo};
use zingo_status::confirmation_status::ConfirmationStatus;
use zingoconfig::ZingoConfig;

use super::TransactionContext;

impl TransactionContext {
    #[allow(clippy::too_many_arguments)]
    async fn execute_bundlescans_internal(
        &self,
        transaction: &Transaction,
        status: ConfirmationStatus,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        outgoing_metadatas: &mut Vec<OutgoingTxData>,
        arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
        taddrs_set: &HashSet<String>,
    ) {
        //todo: investigate scanning all bundles simultaneously

        self.scan_transparent_bundle(
            transaction,
            status,
            block_time,
            is_outgoing_transaction,
            taddrs_set,
        )
        .await;
        self.scan_sapling_bundle(
            transaction,
            status,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
            arbitrary_memos_with_txids,
        )
        .await;
        self.scan_orchard_bundle(
            transaction,
            status,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
            arbitrary_memos_with_txids,
        )
        .await;
    }
    pub(crate) async fn scan_full_tx(
        &self,
        transaction: &Transaction,
        status: ConfirmationStatus,
        block_time: u32,
        price: Option<f64>,
    ) {
        // Set up data structures to record scan results
        let mut txid_indexed_zingo_memos = Vec::new();
        // Remember if this is an outgoing Tx. Useful for when we want to grab the outgoing metadata.
        let mut is_outgoing_transaction = false;
        // Collect our t-addresses for easy checking
        let taddrs_set = self.key.get_all_taddrs(&self.config);
        // Process t-address outputs
        // If this transaction in outgoing, i.e., we received sent some money in this transaction, then we need to grab all transparent outputs
        // that don't belong to us as the outgoing metadata
        // the assumption is either we already decrypted a compact output and filled in some data
        // or transparent something
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
            transaction,
            status,
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
                                transaction
                                    .outgoing_tx_data
                                    .iter_mut()
                                    .filter(|out_meta| {
                                        outgoing_potential_receivers
                                            .contains(&Some(out_meta.to_address.clone()))
                                    })
                                    .for_each(|out_metadata| {
                                        out_metadata.recipient_ua =
                                            Some(ua.encode(&self.config.chain))
                                    })
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
        status: ConfirmationStatus,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        taddrs_set: &HashSet<String>,
    ) {
        // Scan all transparent outputs to see if we received any money
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
                                status,
                                block_time as u64,
                                vout,
                                n as u32,
                            );
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
                            .transparent_notes
                            .iter()
                            .find(|u| u.txid == prev_transaction_id && u.output_index == prev_n)
                        {
                            total_transparent_value_spent += spent_utxo.value;
                            spent_utxos.push((
                                prev_transaction_id,
                                prev_n as u32,
                                transaction.txid(),
                            ));
                        }
                    }
                }
            }
        }

        // Mark all the UTXOs that were spent here back in their original txns.
        for (prev_transaction_id, prev_n, transaction_id) in spent_utxos {
            // Mark that this Tx spent some funds
            *is_outgoing_transaction = true;

            self.transaction_metadata_set
                .write()
                .await
                .mark_txid_utxo_spent(prev_transaction_id, prev_n, transaction_id, status);
        }

        // If this transaction spent value, add the spent amount to the TxID
        if total_transparent_value_spent > 0 {
            *is_outgoing_transaction = true;

            self.transaction_metadata_set.write().await.add_taddr_spent(
                transaction.txid(),
                status,
                block_time as u64,
                total_transparent_value_spent,
            );
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn scan_sapling_bundle(
        &self,
        transaction: &Transaction,
        status: ConfirmationStatus,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        outgoing_metadatas: &mut Vec<OutgoingTxData>,
        arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
    ) {
        self.scan_bundle::<SaplingDomain>(
            transaction,
            status,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
            arbitrary_memos_with_txids,
            false,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn scan_orchard_bundle(
        &self,
        transaction: &Transaction,
        status: ConfirmationStatus,
        block_time: u32,
        is_outgoing_transaction: &mut bool,
        outgoing_metadatas: &mut Vec<OutgoingTxData>,
        arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
    ) {
        self.scan_bundle::<OrchardDomain>(
            transaction,
            status,
            block_time,
            is_outgoing_transaction,
            outgoing_metadatas,
            arbitrary_memos_with_txids,
            true,
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
        status: ConfirmationStatus,
        block_time: u32,
        is_outgoing_transaction: &mut bool, // Isn't this also NA for unconfirmed?
        outgoing_metadatas: &mut Vec<OutgoingTxData>,
        arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
        scan_internal_scope_if_outgoing: bool,
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
        if status.is_broadcast() {
            let unspent_nullifiers = self
                .transaction_metadata_set
                .read()
                .await
                .get_nullifier_value_txid_outputindex_of_unspent_notes::<D>();
            for output in <FnGenBundle<D> as zingo_traits::Bundle<D>>::from_transaction(transaction)
                .into_iter()
                .flat_map(|bundle| bundle.spend_elements().into_iter())
            {
                if let Some((nf, _value, transaction_id, output_index)) = unspent_nullifiers
                    .iter()
                    .find(|(nf, _, _, _)| nf == output.nullifier())
                {
                    let _ = self
                        .transaction_metadata_set
                        .write()
                        .await
                        .found_spent_nullifier(
                            transaction.txid(),
                            status,
                            block_time,
                            (*nf).into(),
                            *transaction_id,
                            *output_index,
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
        let domain_tagged_outputs =
            <FnGenBundle<D> as zingo_traits::Bundle<D>>::from_transaction(transaction)
                .into_iter()
                .flat_map(|bundle| bundle.output_elements().into_iter())
                .map(|output| {
                    (
                        output.domain(status.get_height(), self.config.chain),
                        output.clone(),
                    )
                })
                .collect::<Vec<_>>();

        let (Ok(external_incoming_viewing_key), Ok(external_outgoing_viewing_key)) =
            (D::wc_to_ivk(&self.key), D::wc_to_ovk(&self.key))
        else {
            // skip scanning if wallet has not viewing capability
            return;
        };

        decrypt_and_record_incoming_transactions::<D>(
            external_incoming_viewing_key,
            &domain_tagged_outputs,
            transaction,
            status,
            block_time,
            self,
            arbitrary_memos_with_txids,
        )
        .await;

        for (_domain, output) in domain_tagged_outputs {
            outgoing_metadatas.extend(
                match try_output_recovery_with_ovk::<
                    D,
                    <FnGenBundle<D> as zingo_traits::Bundle<D>>::Output,
                >(
                    &output.domain(status.get_height(), self.config.chain),
                    &external_outgoing_viewing_key,
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
                                if self.key.addresses().iter().any(|unified_address| {
                                    [
                                        unified_address.transparent().cloned().map(Address::from),
                                        unified_address.sapling().cloned().map(Address::from),
                                        unified_address.orchard().cloned().map(
                                            |orchard_receiver| {
                                                Address::from(
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
                                }) {
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

        // now we have decrypted everything about the transaction except anything to do with the internal incoming viewing key. in the intercompatibility case, we may want to check the internal incoming viewing key for sapling. in the present case, i am going to write fast code that doesnt involve rewriting this entire module. the goal is to get incoming change compatibility with zip317 -fv
        if scan_internal_scope_if_outgoing && *is_outgoing_transaction {
            let (Ok(external_incoming_viewing_key), Ok(external_outgoing_viewing_key)) =
                (D::wc_to_ivk(&self.key), D::wc_to_ovk(&self.key))
            else {
                // skip scanning if wallet has not viewing capability
                return;
            };
        }
    }
}

pub async fn decrypt_and_record_incoming_transactions<D>(
    incoming_viewing_key: <D as Domain>::IncomingViewingKey,
    domain_tagged_outputs: &Vec<(D, <<D as DomainWalletExt>::Bundle as Bundle<D>>::Output)>,
    transaction: &Transaction,
    status: ConfirmationStatus,
    block_time: u32,
    records: &TransactionContext,
    arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
) where
    D: DomainWalletExt,
    D::Note: Clone + PartialEq,
    D::OutgoingViewingKey: std::fmt::Debug,
    D::Recipient: zingo_traits::Recipient,
    D::Memo: zingo_traits::ToBytes<512>,
{
    let decrypt_attempts = zcash_note_encryption::batch::try_note_decryption(
        &[incoming_viewing_key],
        &domain_tagged_outputs,
    )
    .into_iter()
    .enumerate();
    for (output_index, decrypt_attempt) in decrypt_attempts {
        let ((note, to, memo_bytes), _ivk_num) = match decrypt_attempt {
            Some(plaintext) => plaintext,
            _ => continue,
        };
        let memo_bytes = MemoBytes::from_bytes(&memo_bytes.to_bytes()).unwrap();
        if let Some(height) = status.get_broadcast_height() {
            records
                .transaction_metadata_set
                .write()
                .await
                .add_pending_note::<D>(
                    transaction.txid(),
                    height,
                    block_time as u64,
                    note.clone(),
                    to,
                    output_index,
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
                Err(e) => {
                    let _memo_error: ZingoLibResult<()> =
                        ZingoLibError::CouldNotDecodeMemo(e).handle();
                }
            }
        }
        records
            .transaction_metadata_set
            .write()
            .await
            .add_memo_to_note_metadata::<D::WalletNote>(&transaction.txid(), note, memo);
    }
}

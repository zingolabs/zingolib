//! TODO: Add Mod Description Here!

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::ZingoConfig;
use zcash_client_backend::ShieldedProtocol;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use crate::wallet::{keys::unified::WalletCapability, tx_map::TxMap};

/// All the data you need to propose, build, and sign a transaction.
#[derive(Clone)]
pub struct TransactionContext {
    /// TODO: Add Doc Comment Here!
    pub config: ZingoConfig,
    /// TODO: Add Doc Comment Here!
    pub(crate) key: Arc<WalletCapability>,
    /// TODO: Add Doc Comment Here!
    pub transaction_metadata_set: Arc<RwLock<TxMap>>,
}

impl TransactionContext {
    /// TODO: Add Doc Comment Here!
    pub fn new(
        config: &ZingoConfig,
        key: Arc<WalletCapability>,
        transaction_metadata_set: Arc<RwLock<TxMap>>,
    ) -> Self {
        Self {
            config: config.clone(),
            key,
            transaction_metadata_set,
        }
    }

    /// returns any outdated records that need to be rescanned for completeness..
    /// checks that each record contains output indexes for its notes
    pub async fn unindexed_records(
        &self,
        wallet_height: BlockHeight,
    ) -> Result<(), Vec<(TxId, BlockHeight)>> {
        let tmdsrl = &self
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id;
        tmdsrl
            .get_spendable_note_ids_and_values(
                &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                wallet_height,
                &[],
            )
            .map(|_| ())
            .map_err(|mut vec| {
                vec.extend_from_slice(&tmdsrl.missing_outgoing_output_indexes());
                vec
            })
    }
}

/// These functions are responsible for receiving a full Transaction and storing it, with a few major caveats.
///  The first layer is CompactTransaction.
///  See trial_decrypt_domain_specific_outputs in some cases, including:
///    * send
///    * read memos
///    * discover outgoing transaction
///    * (mark change / scan for internal change)
///
///  Additional information and processing are required some of the steps in scan_full_tx are similar to or
///  repeat steps in trial_ddso however, scan_full_tx is more limited than trial_ddso:
///    * scan_full_tx has no access to a transmitter. So it is incapable of **sending a request on a transmitter for another task to fetch a witnessed position**.
///
/// Unlike a viewkey wallet, a spendkey wallet MUST pass reread the block to find a witnessed position to pass to add_new_note. scan_full_tx cannot do this.
/// thus, scan_full_tx is incomplete and skips some steps on the assumption that they will be covered elsewhere. Notably, add_note is not called inside scan_full_tx.
/// (A viewkey wallet, on the other hand, doesnt need witness and could maybe get away with only calling scan_full_tx)
mod decrypt_transaction {
    use crate::{
        error::{ZingoLibError, ZingoLibResult},
        utils::interpret_taddr_as_tex_addr,
        wallet::{
            self,
            data::OutgoingTxData,
            keys::unified::{External, Fvk, Ivk},
            notes::{ShieldedNoteInterface, TransparentOutput},
            traits::{
                self as zingo_traits, Bundle as _, DomainWalletExt, Recipient as _,
                ShieldedOutputExt as _, Spend as _, ToBytes as _,
            },
            transaction_record::TransactionKind,
        },
    };
    use orchard::note_encryption::OrchardDomain;
    use sapling_crypto::note_encryption::SaplingDomain;
    use std::convert::TryInto;

    use zcash_client_backend::address::{Address, UnifiedAddress};
    use zcash_note_encryption::{try_output_recovery_with_ovk, Domain};
    use zcash_primitives::{
        legacy::TransparentAddress,
        memo::{Memo, MemoBytes},
        transaction::{
            components::{transparent::Authorized, TxIn},
            Transaction, TxId,
        },
    };
    use zingo_memo::{parse_zingo_memo, ParsedMemo};
    use zingo_status::confirmation_status::ConfirmationStatus;

    use super::TransactionContext;

    impl TransactionContext {
        /// This method records that receipt in the relevant receiving
        /// TransactionRecord in the TransactionRecordsById database.
        async fn record_taddr_receipt(
            &self,
            transaction: &zcash_primitives::transaction::Transaction,
            status: ConfirmationStatus,
            output_taddr: String,
            block_time: Option<u32>,
            vout: &zcash_primitives::transaction::components::TxOut,
            n: usize,
        ) {
            self.transaction_metadata_set
                .write()
                .await
                .transaction_records_by_id
                .add_new_taddr_output(
                    transaction.txid(),
                    output_taddr.clone(),
                    status,
                    block_time,
                    vout,
                    n as u32,
                );
        }
        async fn get_spent_utxo(
            &self,
            vin: &TxIn<Authorized>,
        ) -> Option<(TransparentOutput, TxId, u64)> {
            let current_transaction_records_by_id = &self
                .transaction_metadata_set
                .read()
                .await
                .transaction_records_by_id;
            let prev_transaction_id = TxId::from_bytes(*vin.prevout.hash());
            let prev_n = vin.prevout.n() as u64;
            if let Some(prev_tx_record) =
                current_transaction_records_by_id.get(&prev_transaction_id)
            {
                // One of the tx outputs is a match
                prev_tx_record
                    .transparent_outputs
                    .iter()
                    .find(|u| u.txid == prev_transaction_id && u.output_index == prev_n)
                    .map(|spent_utxo| (spent_utxo.clone(), prev_transaction_id, prev_n))
            } else {
                None
            }
        }
        // Za has ds-integration with the name "ephemeral" address.
        fn identify_rejection_address(&self, spent_utxo: TransparentOutput) -> Option<String> {
            if self
                .key
                .get_rejection_address_set(&self.config.chain)
                .contains(&spent_utxo.address)
            {
                Some(spent_utxo.address)
            } else {
                None
            }
        }

        /// account here is a verb meaning record note data
        /// and perform some other appropriate actions
        async fn account_for_shielded_receipts<D>(
            &self,
            ivk: Ivk<D, External>,
            domain_tagged_outputs: &[(
                D,
                <<D as DomainWalletExt>::Bundle as wallet::traits::Bundle<D>>::Output,
            )],
            status: ConfirmationStatus,
            transaction: &Transaction,
            block_time: Option<u32>,
            arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
        ) where
            D: zingo_traits::DomainWalletExt,
            <D as Domain>::Recipient: wallet::traits::Recipient,
            <D as Domain>::Note: PartialEq,
            <D as Domain>::Note: Clone,
            D::Memo: zingo_traits::ToBytes<512>,
        {
            let decrypt_attempts = zcash_note_encryption::batch::try_note_decryption(
                &[ivk.ivk],
                domain_tagged_outputs,
            )
            .into_iter()
            .enumerate();
            for (output_index, decrypt_attempt) in decrypt_attempts {
                let ((note, to, memo_bytes), _ivk_num) = match decrypt_attempt {
                    Some(plaintext) => plaintext,
                    _ => continue,
                };
                let memo_bytes = MemoBytes::from_bytes(&memo_bytes.to_bytes()).unwrap();
                // if status is pending add the whole pending note
                // otherwise, just update the output index

                let tx_map = &mut self
                    .transaction_metadata_set
                    .write()
                    .await
                    .transaction_records_by_id;

                let transaction_record = tx_map.create_modify_get_transaction_record(
                    &transaction.txid(),
                    status,
                    block_time,
                );

                if !status.is_confirmed() {
                    transaction_record.add_pending_note::<D>(note.clone(), to, output_index);
                } else {
                    let _note_does_not_exist_result =
                        transaction_record.update_output_index::<D>(note.clone(), output_index);
                }

                let memo = memo_bytes
                    .clone()
                    .try_into()
                    .unwrap_or(Memo::Future(memo_bytes));
                if let Memo::Arbitrary(ref wallet_internal_data) = memo {
                    match parse_zingo_memo(*wallet_internal_data.as_ref()) {
                        Ok(parsed_zingo_memo) => {
                            arbitrary_memos_with_txids
                                .push((parsed_zingo_memo, transaction.txid()));
                        }
                        Err(e) => {
                            let _memo_error: ZingoLibResult<()> =
                                ZingoLibError::CouldNotDecodeMemo(e).handle();
                        }
                    }
                }
                tx_map.add_memo_to_note_metadata::<D::WalletNote>(&transaction.txid(), note, memo);
            }
        }
        /// Transactions contain per-protocol "bundles" of components.
        /// The component details vary by protocol.
        /// In Sapling the components are "Spends" and "Outputs"
        /// In Orchard the components are "Actions", each of which
        /// _IS_ 1 Spend and 1 Output.
        #[allow(clippy::too_many_arguments)]
        async fn decrypt_transaction_to_record_domain<D: DomainWalletExt>(
            &self,
            transaction: &Transaction,
            status: ConfirmationStatus,
            block_time: Option<u32>,
            outgoing_metadatas: &mut Vec<OutgoingTxData>,
            arbitrary_memos_with_txids: &mut Vec<(ParsedMemo, TxId)>,
        ) {
            type FnGenBundle<I> = <I as DomainWalletExt>::Bundle;
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

            let Ok(fvk) = D::unified_key_store_to_fvk(&self.key.unified_key_store) else {
                // skip scanning if wallet has no viewing capability
                return;
            };
            let (ivk, ovk) = (fvk.derive_ivk::<External>(), fvk.derive_ovk::<External>());

            self.account_for_shielded_receipts(
                ivk,
                &domain_tagged_outputs,
                status,
                transaction,
                block_time,
                arbitrary_memos_with_txids,
            )
            .await;
            // Check if any of the nullifiers generated in this transaction are ours. We only need this for pending transactions,
            // because for transactions in the block, we will check the nullifiers from the blockdata
            if !status.is_confirmed() {
                let unspent_nullifiers = self
                    .transaction_metadata_set
                    .read()
                    .await
                    .get_nullifier_value_txid_outputindex_of_unspent_notes::<D>();
                for output in
                    <FnGenBundle<D> as zingo_traits::Bundle<D>>::from_transaction(transaction)
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
            for (i, (_domain, output)) in domain_tagged_outputs.iter().enumerate() {
                outgoing_metadatas.extend(
                    match try_output_recovery_with_ovk::<
                        D,
                        <FnGenBundle<D> as zingo_traits::Bundle<D>>::Output,
                    >(
                        &output.domain(status.get_height(), self.config.chain),
                        &ovk.ovk,
                        output,
                        &output.value_commitment(),
                        &output.out_ciphertext(),
                    ) {
                        Some((note, payment_address, memo_bytes)) => {
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
                                            unified_address
                                                .transparent()
                                                .cloned()
                                                .map(Address::from),
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
                                        None
                                    } else {
                                        Some(OutgoingTxData {
                                            recipient_address: address,
                                            value: D::WalletNote::value_from_note(&note),
                                            memo,
                                            recipient_ua: None,
                                            output_index: Some(
                                                D::output_index_offset(transaction) + i as u64,
                                            ),
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
}

//! contains associated methods for asking TxMap about the data it contains
//! Does not contain trait implementations

use zcash_note_encryption::Domain;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use crate::wallet::notes::interface::OutputConstructor;
use crate::wallet::{
    data::{PoolNullifier, TransactionRecord},
    notes::OutputInterface,
    notes::{query::OutputSpendStatusQuery, ShieldedNoteInterface},
    traits::{DomainWalletExt, Recipient},
};

use super::TxMap;

impl TxMap {
    /// TODO: Doc-comment!
    pub fn get_notes_for_updating(
        &self,
        before_block: u64,
    ) -> Vec<(TxId, PoolNullifier, Option<u32>)> {
        let before_block = BlockHeight::from_u32(before_block as u32);

        self.transaction_records_by_id
            .iter()
            .filter(|(_, transaction_metadata)| transaction_metadata.status.is_confirmed_before_or_at(&before_block)) // Update only confirmed notes
            .flat_map(|(txid, transaction_metadata)| {
                // Fetch notes that are before the before_block.
                transaction_metadata
                    .sapling_notes
                    .iter()
                    .filter_map(move |sapling_note_description| {
                        if sapling_note_description.have_spending_key
                            && !sapling_note_description.is_spent_confirmed()
                        {
                            Some((
                                *txid,
                                PoolNullifier::Sapling(
                                    sapling_note_description.nullifier.unwrap_or_else(|| {
                                        todo!("Do something about note even with missing nullifier")
                                    }),
                                ),
                                sapling_note_description.output_index
                            ))
                        } else {
                            None
                        }
                    })
                    .chain(transaction_metadata.orchard_notes.iter().filter_map(
                        move |orchard_note_description| {
                            if orchard_note_description.have_spending_key
                                && !orchard_note_description.is_spent_confirmed()
                            {
                                Some((
                                    *txid,
                                    PoolNullifier::Orchard(orchard_note_description.nullifier.unwrap_or_else(|| {
                                        todo!("Do something about note even with missing nullifier")
                                    }))
                                    , orchard_note_description.output_index
,                                ))
                            } else {
                                None
                            }
                        },
                    ))
            })
            .collect()
    }

    /// TODO: Doc-comment!
    #[allow(clippy::type_complexity)]
    pub fn get_nullifier_value_txid_outputindex_of_unspent_notes<D: DomainWalletExt>(
        &self,
    ) -> Vec<(
        <<D as DomainWalletExt>::WalletNote as ShieldedNoteInterface>::Nullifier,
        u64,
        TxId,
        Option<u32>,
    )>
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: Recipient,
    {
        self.transaction_records_by_id
            .iter()
            .flat_map(|(_, transaction_record)| {
                D::WalletNote::get_record_query_matching_outputs(
                    transaction_record,
                    OutputSpendStatusQuery::only_unspent(),
                )
                .iter()
                .filter_map(move |unspent_note_data| {
                    unspent_note_data.nullifier().map(|unspent_nullifier| {
                        (
                            unspent_nullifier,
                            unspent_note_data.value(),
                            transaction_record.txid,
                            *unspent_note_data.output_index(),
                        )
                    })
                })
                .collect::<Vec<(
                    <<D as DomainWalletExt>::WalletNote as ShieldedNoteInterface>::Nullifier,
                    u64,
                    TxId,
                    Option<u32>,
                )>>()
            })
            .collect()
    }

    /// This returns an _arbitrary_ confirmed txid from the latest block the wallet is aware of.
    pub fn get_some_txid_from_highest_wallet_block(&self) -> Option<TxId> {
        self.transaction_records_by_id
            .values()
            .fold(
                None,
                |highest: Option<(TxId, BlockHeight)>, w: &TransactionRecord| match w
                    .status
                    .get_confirmed_height()
                {
                    None => highest,
                    Some(w_height) => match highest {
                        None => Some((w.txid, w_height)),
                        Some(highest_tuple) => {
                            if highest_tuple.1 > w_height {
                                highest
                            } else {
                                Some((w.txid, w_height))
                            }
                        }
                    },
                },
            )
            .map(|v| v.0)
    }
}

#[test]
fn test_get_some_txid_from_highest_wallet_block() {
    let mut tms = TxMap::new_treeless_address_free();
    assert_eq!(tms.get_some_txid_from_highest_wallet_block(), None);
    let txid_bytes_1 = [0u8; 32];
    let txid_bytes_2 = [1u8; 32];
    let txid_bytes_3 = [2u8; 32];
    let txid_1 = TxId::from_bytes(txid_bytes_1);
    let txid_2 = TxId::from_bytes(txid_bytes_2);
    let txid_3 = TxId::from_bytes(txid_bytes_3);
    tms.transaction_records_by_id
        .insert_transaction_record(TransactionRecord::new(
            zingo_status::confirmation_status::ConfirmationStatus::Mempool(BlockHeight::from_u32(
                3_200_000,
            )),
            100,
            &txid_1,
        ));
    tms.transaction_records_by_id
        .insert_transaction_record(TransactionRecord::new(
            zingo_status::confirmation_status::ConfirmationStatus::Confirmed(
                BlockHeight::from_u32(3_000_069),
            ),
            0,
            &txid_2,
        ));
    tms.transaction_records_by_id
        .insert_transaction_record(TransactionRecord::new(
            zingo_status::confirmation_status::ConfirmationStatus::Confirmed(
                BlockHeight::from_u32(2_650_000),
            ),
            0,
            &txid_3,
        ));
    let highest = tms.get_some_txid_from_highest_wallet_block();
    assert_eq!(highest, Some(txid_2));
}

#[cfg(feature = "lightclient-deprecated")]
impl TxMap {
    /// TODO: Doc-comment!
    pub fn get_fee_by_txid(&self, txid: &TxId) -> u64 {
        let transaction_record = self
            .transaction_records_by_id
            .get(txid)
            .expect("should have the requested transaction record in the wallet");
        match self
            .transaction_records_by_id
            .calculate_transaction_fee(transaction_record)
        {
            Ok(tx_fee) => tx_fee,
            Err(e) => panic!("{:?} for txid {}", e, txid,),
        }
    }
}

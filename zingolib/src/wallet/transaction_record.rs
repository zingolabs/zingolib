//! An (incomplete) representation of what the Zingo instance "knows" about a transaction
//! conspicuously absent is the set of transparent inputs to the transaction.
//! by its`nature this evolves through, different states of completeness.
use std::io::{self, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt as _, WriteBytesExt as _};
use incrementalmerkletree::witness::IncrementalWitness;
use orchard::tree::MerkleHashOrchard;
use zcash_client_backend::PoolType;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::TxId;

use crate::error::ZingoLibError;
use crate::wallet::notes;

use super::{
    data::{OutgoingTxData, PoolNullifier, COMMITMENT_TREE_LEVELS},
    keys::unified::WalletCapability,
    notes::{NoteRecordIdentifier, ShieldedNoteInterface},
    traits::{DomainWalletExt, ReadableWriteable as _},
};

///  Everything (SOMETHING) about a transaction
#[derive(Debug)]
pub struct TransactionRecord {
    /// the relationship of the transaction to the blockchain. can be either Broadcast (to mempool}, or Confirmed.
    pub status: zingo_status::confirmation_status::ConfirmationStatus,

    /// Timestamp of Tx. Added in v4
    pub datetime: u64,

    /// Txid of this transaction. It's duplicated here (It is also the Key in the HashMap that points to this
    /// WalletTx in LightWallet::txs)
    pub transaction_id: TxId,

    /// List of all nullifiers spent by this wallet in this Tx.
    pub spent_sapling_nullifiers: Vec<sapling_crypto::Nullifier>,

    /// List of all nullifiers spent by this wallet in this Tx. These nullifiers belong to the wallet.
    pub spent_orchard_nullifiers: Vec<orchard::note::Nullifier>,

    /// List of all sapling notes received by this wallet in this tx. Some of these might be change notes.
    pub sapling_notes: Vec<notes::SaplingNote>,

    /// List of all sapling notes received by this wallet in this tx. Some of these might be change notes.
    pub orchard_notes: Vec<notes::OrchardNote>,

    /// List of all Utxos by this wallet received in this Tx. Some of these might be change notes
    pub transparent_notes: Vec<notes::TransparentNote>,

    /// Total value of all the sapling nullifiers that were spent by this wallet in this Tx
    pub total_sapling_value_spent: u64,

    /// Total value of all the orchard nullifiers that were spent by this wallet in this Tx
    pub total_orchard_value_spent: u64,

    /// Total amount of transparent funds that belong to us that were spent by this wallet in this Tx.
    pub total_transparent_value_spent: u64,

    /// All outgoing sends
    pub outgoing_tx_data: Vec<OutgoingTxData>,

    /// Price of Zec when this Tx was created
    pub price: Option<f64>,
}

// set
impl TransactionRecord {
    /// TODO: Add Doc Comment Here!
    pub fn new(
        status: zingo_status::confirmation_status::ConfirmationStatus,
        datetime: u64,
        transaction_id: &TxId,
    ) -> Self {
        TransactionRecord {
            status,
            datetime,
            transaction_id: *transaction_id,
            spent_sapling_nullifiers: vec![],
            spent_orchard_nullifiers: vec![],
            sapling_notes: vec![],
            orchard_notes: vec![],
            transparent_notes: vec![],
            total_transparent_value_spent: 0,
            total_sapling_value_spent: 0,
            total_orchard_value_spent: 0,
            outgoing_tx_data: vec![],
            price: None,
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn add_spent_nullifier(&mut self, nullifier: PoolNullifier, value: u64) {
        match nullifier {
            PoolNullifier::Sapling(sapling_nullifier) => {
                self.spent_sapling_nullifiers.push(sapling_nullifier);
                self.total_sapling_value_spent += value;
            }
            PoolNullifier::Orchard(orchard_nullifier) => {
                self.spent_orchard_nullifiers.push(orchard_nullifier);
                self.total_orchard_value_spent += value;
            }
        }
    }
    // much data assignment of this struct is done through the pub fields as of january 2024. Todo: should have private fields and public methods.
}
//get
impl TransactionRecord {
    /// TODO: Add Doc Comment Here!
    pub fn get_transparent_value_spent(&self) -> u64 {
        self.total_transparent_value_spent
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_transaction_fee(&self) -> Result<u64, ZingoLibError> {
        let outputted = self.value_outgoing() + self.total_change_returned();
        if self.total_value_spent() >= outputted {
            Ok(self.total_value_spent() - outputted)
        } else {
            ZingoLibError::MetadataUnderflow(format!(
                "for txid {} with status {}: spent {}, outgoing {}, returned change {} \n {:?}",
                self.transaction_id,
                self.status,
                self.total_value_spent(),
                self.value_outgoing(),
                self.total_change_returned(),
                self,
            ))
            .handle()
        }
    }

    /// For each Shielded note received in this transactions,
    /// pair it with a NoteRecordIdentifier identifying the note
    /// and return the list
    // TODO: Make these generic, this is wet code
    pub fn select_unspent_note_noteref_pairs<D>(
        &self,
    ) -> Vec<(
        <D as zcash_note_encryption::Domain>::Note,
        NoteRecordIdentifier,
    )>
    where
        D: DomainWalletExt,
        <D as zcash_note_encryption::Domain>::Note: PartialEq + Clone,
        <D as zcash_note_encryption::Domain>::Recipient: super::traits::Recipient,
    {
        let mut value_ref_pairs = Vec::new();
        <D as DomainWalletExt>::to_notes_vec(self)
            .iter()
            .for_each(|note| {
                if !notes::NoteInterface::is_spent_or_pending_spent(note) {
                    if let Some(index) = note.output_index() {
                        let index = *index;
                        let note_record_reference = NoteRecordIdentifier {
                            txid: self.transaction_id,
                            pool: PoolType::Shielded(
                                zcash_client_backend::ShieldedProtocol::Sapling,
                            ),
                            index,
                        };
                        value_ref_pairs.push((
                            notes::ShieldedNoteInterface::note(note).clone(),
                            note_record_reference,
                        ));
                    }
                }
            });
        value_ref_pairs
    }
    /// TODO: Add Doc Comment Here!
    // TODO: This is incorrect in the edge case where where we have a send-to-self with
    // no text memo and 0-value fee
    pub fn is_outgoing_transaction(&self) -> bool {
        (!self.outgoing_tx_data.is_empty()) || self.total_value_spent() != 0
    }

    /// TODO: Add Doc Comment Here!
    pub fn is_incoming_transaction(&self) -> bool {
        self.sapling_notes
            .iter()
            .any(|note| !notes::ShieldedNoteInterface::is_change(note))
            || self
                .orchard_notes
                .iter()
                .any(|note| !notes::ShieldedNoteInterface::is_change(note))
            || !self.transparent_notes.is_empty()
    }

    /// TODO: Add Doc Comment Here!
    pub fn net_spent(&self) -> u64 {
        assert!(self.is_outgoing_transaction());
        self.total_value_spent() - self.total_change_returned()
    }

    /// TODO: Add Doc Comment Here!
    fn pool_change_returned<D: DomainWalletExt>(&self) -> u64
    where
        <D as zcash_note_encryption::Domain>::Note: PartialEq + Clone,
        <D as zcash_note_encryption::Domain>::Recipient: super::traits::Recipient,
    {
        D::sum_pool_change(self)
    }

    /// TODO: Add Doc Comment Here!
    pub fn pool_value_received<D: DomainWalletExt>(&self) -> u64
    where
        <D as zcash_note_encryption::Domain>::Note: PartialEq + Clone,
        <D as zcash_note_encryption::Domain>::Recipient: super::traits::Recipient,
    {
        D::to_notes_vec(self)
            .iter()
            .map(|note_and_metadata| note_and_metadata.value())
            .sum()
    }

    /// TODO: Add Doc Comment Here!
    pub fn total_change_returned(&self) -> u64 {
        self.pool_change_returned::<sapling_crypto::note_encryption::SaplingDomain>()
            + self.pool_change_returned::<orchard::note_encryption::OrchardDomain>()
    }

    /// TODO: Add Doc Comment Here!
    pub fn total_value_received(&self) -> u64 {
        self.pool_value_received::<orchard::note_encryption::OrchardDomain>()
            + self.pool_value_received::<sapling_crypto::note_encryption::SaplingDomain>()
            + self
                .transparent_notes
                .iter()
                .map(|utxo| utxo.value)
                .sum::<u64>()
    }

    /// TODO: Add Doc Comment Here!
    pub fn total_value_spent(&self) -> u64 {
        self.value_spent_by_pool().iter().sum()
    }

    /// TODO: Add Doc Comment Here!
    pub fn value_outgoing(&self) -> u64 {
        self.outgoing_tx_data
            .iter()
            .fold(0, |running_total, tx_data| tx_data.value + running_total)
    }

    /// TODO: Add Doc Comment Here!
    pub fn value_spent_by_pool(&self) -> [u64; 3] {
        [
            self.get_transparent_value_spent(),
            self.total_sapling_value_spent,
            self.total_orchard_value_spent,
        ]
    }

    /// Gets a received note, by index and domain
    pub fn get_received_note<D>(
        &self,
        index: u32,
    ) -> Option<
        zcash_client_backend::wallet::ReceivedNote<
            NoteRecordIdentifier,
            <D as zcash_note_encryption::Domain>::Note,
        >,
    >
    where
        D: DomainWalletExt + Sized,
        D::Note: PartialEq + Clone,
        D::Recipient: super::traits::Recipient,
    {
        let note = D::to_notes_vec(self)
            .iter()
            .find(|note| *note.output_index() == Some(index));
        note.and_then(|note| {
            let txid = self.transaction_id;
            let note_record_reference = NoteRecordIdentifier {
                txid,
                pool: zcash_client_backend::PoolType::Shielded(note.to_zcb_note().protocol()),
                index,
            };
            note.witnessed_position().map(|pos| {
                zcash_client_backend::wallet::ReceivedNote::from_parts(
                    note_record_reference,
                    txid,
                    index as u16,
                    note.note().clone(),
                    zip32::Scope::External,
                    pos,
                )
            })
        })
    }
}
// read/write
impl TransactionRecord {
    /// TODO: Add Doc Comment Here!
    #[allow(clippy::type_complexity)]
    pub fn read<R: Read>(
        mut reader: R,
        (wallet_capability, mut trees): (
            &WalletCapability,
            Option<&mut (
                Vec<(
                    IncrementalWitness<sapling_crypto::Node, COMMITMENT_TREE_LEVELS>,
                    BlockHeight,
                )>,
                Vec<(
                    IncrementalWitness<MerkleHashOrchard, COMMITMENT_TREE_LEVELS>,
                    BlockHeight,
                )>,
            )>,
        ),
    ) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;

        let block = BlockHeight::from_u32(reader.read_i32::<LittleEndian>()? as u32);

        let unconfirmed = if version <= 20 {
            false
        } else {
            reader.read_u8()? == 1
        };

        let datetime = if version >= 4 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };

        let mut transaction_id_bytes = [0u8; 32];
        reader.read_exact(&mut transaction_id_bytes)?;

        let transaction_id = TxId::from_bytes(transaction_id_bytes);

        let sapling_notes = zcash_encoding::Vector::read_collected_mut(&mut reader, |r| {
            notes::SaplingNote::read(r, (wallet_capability, trees.as_mut().map(|t| &mut t.0)))
        })?;
        let orchard_notes = if version > 22 {
            zcash_encoding::Vector::read_collected_mut(&mut reader, |r| {
                notes::OrchardNote::read(r, (wallet_capability, trees.as_mut().map(|t| &mut t.1)))
            })?
        } else {
            vec![]
        };
        let utxos = zcash_encoding::Vector::read(&mut reader, |r| notes::TransparentNote::read(r))?;

        let total_sapling_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_transparent_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_orchard_value_spent = if version >= 22 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };

        // Outgoing metadata was only added in version 2
        let outgoing_metadata =
            zcash_encoding::Vector::read(&mut reader, |r| OutgoingTxData::read(r))?;

        let _full_tx_scanned = reader.read_u8()? > 0;

        let zec_price = if version <= 4 {
            None
        } else {
            zcash_encoding::Optional::read(&mut reader, |r| r.read_f64::<LittleEndian>())?
        };

        let spent_sapling_nullifiers = if version <= 5 {
            vec![]
        } else {
            zcash_encoding::Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(sapling_crypto::Nullifier(n))
            })?
        };

        let spent_orchard_nullifiers = if version <= 21 {
            vec![]
        } else {
            zcash_encoding::Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(orchard::note::Nullifier::from_bytes(&n).unwrap())
            })?
        };
        let status = zingo_status::confirmation_status::ConfirmationStatus::from_blockheight_and_unconfirmed_bool(block, unconfirmed);
        Ok(Self {
            status,
            datetime,
            transaction_id,
            sapling_notes,
            orchard_notes,
            transparent_notes: utxos,
            spent_sapling_nullifiers,
            spent_orchard_nullifiers,
            total_sapling_value_spent,
            total_transparent_value_spent,
            total_orchard_value_spent,
            outgoing_tx_data: outgoing_metadata,
            price: zec_price,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn serialized_version() -> u64 {
        23
    }

    /// TODO: Add Doc Comment Here!
    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        let block: u32 = self.status.get_height().into();
        writer.write_i32::<LittleEndian>(block as i32)?;

        writer.write_u8(if !self.status.is_confirmed() { 1 } else { 0 })?;

        writer.write_u64::<LittleEndian>(self.datetime)?;

        writer.write_all(self.transaction_id.as_ref())?;

        zcash_encoding::Vector::write(&mut writer, &self.sapling_notes, |w, nd| nd.write(w))?;
        zcash_encoding::Vector::write(&mut writer, &self.orchard_notes, |w, nd| nd.write(w))?;
        zcash_encoding::Vector::write(&mut writer, &self.transparent_notes, |w, u| u.write(w))?;

        for pool in self.value_spent_by_pool() {
            writer.write_u64::<LittleEndian>(pool)?;
        }

        // Write the outgoing metadata
        zcash_encoding::Vector::write(&mut writer, &self.outgoing_tx_data, |w, om| om.write(w))?;

        writer.write_u8(0)?;

        zcash_encoding::Optional::write(&mut writer, self.price, |w, p| {
            w.write_f64::<LittleEndian>(p)
        })?;

        zcash_encoding::Vector::write(&mut writer, &self.spent_sapling_nullifiers, |w, n| {
            w.write_all(&n.0)
        })?;
        zcash_encoding::Vector::write(&mut writer, &self.spent_orchard_nullifiers, |w, n| {
            w.write_all(&n.to_bytes())
        })?;

        Ok(())
    }
}

#[cfg(any(test, feature = "test-features"))]
pub mod mocks {
    //! Mock version of the struct for testing
    use zcash_primitives::transaction::TxId;
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::test_framework::mocks::build_method;

    use super::TransactionRecord;

    /// to create a mock TransactionRecord
    pub struct TransactionRecordBuilder {
        status: Option<ConfirmationStatus>,
        datetime: Option<u64>,
        txid: Option<TxId>,
    }
    #[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
    impl TransactionRecordBuilder {
        /// blank builder
        pub fn new() -> Self {
            Self {
                status: None,
                datetime: None,
                txid: None,
            }
        }
        // Methods to set each field
        build_method!(status, ConfirmationStatus);
        build_method!(datetime, u64);
        build_method!(txid, TxId);

        /// Use the mocery of random_txid to get one?
        pub fn randomize_txid(self) -> Self {
            self.txid(crate::test_framework::mocks::random_txid())
        }

        /// builds a mock TransactionRecord after all pieces are supplied
        pub fn build(self) -> TransactionRecord {
            TransactionRecord::new(
                self.status.unwrap(),
                self.datetime.unwrap(),
                &self.txid.unwrap(),
            )
        }
    }

    impl Default for TransactionRecordBuilder {
        fn default() -> Self {
            Self {
                status: Some(
                    zingo_status::confirmation_status::ConfirmationStatus::Confirmed(
                        zcash_primitives::consensus::BlockHeight::from_u32(5),
                    ),
                ),
                datetime: Some(1705077003),
                txid: Some(crate::test_framework::mocks::default_txid()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::wallet::notes::transparent::mocks::TransparentNoteBuilder;
    use crate::wallet::transaction_record::mocks::TransactionRecordBuilder;

    #[test]
    pub fn blank_record() {
        let new = TransactionRecordBuilder::default().build();
        assert_eq!(new.get_transparent_value_spent(), 0);
        assert_eq!(new.get_transaction_fee().unwrap(), 0);
        assert!(!new.is_outgoing_transaction());
        assert!(!new.is_incoming_transaction());
        // assert_eq!(new.net_spent(), 0);
        assert_eq!(
            new.pool_change_returned::<orchard::note_encryption::OrchardDomain>(),
            0
        );
        assert_eq!(
            new.pool_change_returned::<sapling_crypto::note_encryption::SaplingDomain>(),
            0
        );
        assert_eq!(new.total_value_received(), 0);
        assert_eq!(new.total_value_spent(), 0);
        assert_eq!(new.value_outgoing(), 0);
        let t: [u64; 3] = [0, 0, 0];
        assert_eq!(new.value_spent_by_pool(), t);
    }
    #[test]
    fn single_transparent_note_makes_is_incoming_true() {
        // A single transparent note makes is_incoming_transaction true.
        let mut transaction_record = TransactionRecordBuilder::default().build();
        transaction_record
            .transparent_notes
            .push(TransparentNoteBuilder::default().build());
        assert!(transaction_record.is_incoming_transaction());
    }
}

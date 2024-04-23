//! Data about a particular Transaction

use byteorder::{LittleEndian, ReadBytesExt as _, WriteBytesExt as _};
use std::io::{self, Read, Write};

use incrementalmerkletree::witness::IncrementalWitness;
use orchard::tree::MerkleHashOrchard;
use zcash_client_backend::PoolType;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use crate::error::ZingoLibError;
use crate::wallet::notes::interface::NoteInterface;
use crate::wallet::traits::ReadableWriteable;
use crate::wallet::{
    data::{OutgoingTxData, PoolNullifier, COMMITMENT_TREE_LEVELS},
    keys::unified::WalletCapability,
    notes::{
        query::OutputQuery, OrchardNote, OutputId, SaplingNote, ShieldedNoteInterface,
        TransparentOutput,
    },
    traits::DomainWalletExt,
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
    pub txid: TxId,

    /// List of all nullifiers spent by this wallet in this Tx.
    pub spent_sapling_nullifiers: Vec<sapling_crypto::Nullifier>,

    /// List of all nullifiers spent by this wallet in this Tx. These nullifiers belong to the wallet.
    pub spent_orchard_nullifiers: Vec<orchard::note::Nullifier>,

    /// List of all sapling notes received by this wallet in this tx. Some of these might be change notes.
    pub sapling_notes: Vec<SaplingNote>,

    /// List of all sapling notes received by this wallet in this tx. Some of these might be change notes.
    pub orchard_notes: Vec<OrchardNote>,

    /// List of all Utxos by this wallet received in this Tx. Some of these might be change notes
    pub transparent_outputs: Vec<TransparentOutput>,

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
            txid: *transaction_id,
            spent_sapling_nullifiers: vec![],
            spent_orchard_nullifiers: vec![],
            sapling_notes: vec![],
            orchard_notes: vec![],
            transparent_outputs: vec![],
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
    /// Uses a query to select all notes with specific properties and return a vector of their identifiers
    pub fn query_for_ids(&self, include_notes: OutputQuery) -> Vec<OutputId> {
        let mut set = vec![];
        let spend_status_query = *include_notes.spend_status();
        if *include_notes.transparent() {
            for note in self.transparent_outputs.iter() {
                if note.spend_status_query(spend_status_query) {
                    set.push(OutputId::from_parts(
                        self.txid,
                        PoolType::Transparent,
                        note.output_index as u32,
                    ));
                }
            }
        }
        if *include_notes.sapling() {
            for note in self.sapling_notes.iter() {
                if note.spend_status_query(spend_status_query) {
                    if let Some(output_index) = note.output_index {
                        set.push(OutputId::from_parts(
                            self.txid,
                            PoolType::Transparent,
                            output_index,
                        ));
                    }
                }
            }
        }
        if *include_notes.orchard() {
            for note in self.orchard_notes.iter() {
                if note.spend_status_query(spend_status_query) {
                    if let Some(output_index) = note.output_index {
                        set.push(OutputId::from_parts(
                            self.txid,
                            PoolType::Transparent,
                            output_index,
                        ));
                    }
                }
            }
        }
        set
    }

    /// Uses a query to select all notes with specific properties and sum them
    pub fn query_sum_value(&self, include_notes: OutputQuery) -> u64 {
        let mut sum = 0;
        let spend_status_query = *include_notes.spend_status();
        if *include_notes.transparent() {
            for note in self.transparent_outputs.iter() {
                if note.spend_status_query(spend_status_query) {
                    sum += note.value()
                }
            }
        }
        if *include_notes.sapling() {
            for note in self.sapling_notes.iter() {
                if note.spend_status_query(spend_status_query) {
                    sum += note.value()
                }
            }
        }
        if *include_notes.orchard() {
            for note in self.orchard_notes.iter() {
                if note.spend_status_query(spend_status_query) {
                    sum += note.value()
                }
            }
        }
        sum
    }

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
                self.txid,
                self.status,
                self.total_value_spent(),
                self.value_outgoing(),
                self.total_change_returned(),
                self,
            ))
            .handle()
        }
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
            .any(|note| !ShieldedNoteInterface::is_change(note))
            || self
                .orchard_notes
                .iter()
                .any(|note| !ShieldedNoteInterface::is_change(note))
            || !self.transparent_outputs.is_empty()
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

    /// Sums all the received notes in the transaction.
    pub fn total_value_received(&self) -> u64 {
        self.query_sum_value(OutputQuery::stipulations(
            true, true, true, true, true, true,
        ))
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
            SaplingNote::read(r, (wallet_capability, trees.as_mut().map(|t| &mut t.0)))
        })?;
        let orchard_notes = if version > 22 {
            zcash_encoding::Vector::read_collected_mut(&mut reader, |r| {
                OrchardNote::read(r, (wallet_capability, trees.as_mut().map(|t| &mut t.1)))
            })?
        } else {
            vec![]
        };
        let utxos = zcash_encoding::Vector::read(&mut reader, |r| TransparentOutput::read(r))?;
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
            txid: transaction_id,
            sapling_notes,
            orchard_notes,
            transparent_outputs: utxos,
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

        writer.write_all(self.txid.as_ref())?;

        zcash_encoding::Vector::write(&mut writer, &self.sapling_notes, |w, nd| nd.write(w))?;
        zcash_encoding::Vector::write(&mut writer, &self.orchard_notes, |w, nd| nd.write(w))?;
        zcash_encoding::Vector::write(&mut writer, &self.transparent_outputs, |w, u| u.write(w))?;

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

    use crate::{
        test_framework::mocks::{build_method, default_txid},
        wallet::notes::{
            orchard::mocks::OrchardNoteBuilder, sapling::mocks::SaplingNoteBuilder,
            transparent::mocks::TransparentOutputBuilder,
        },
    };

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

    /// creates a TransactionRecord holding each type of note.
    pub fn nine_note_transaction_record() -> TransactionRecord {
        let spend = Some((default_txid(), 112358));

        let mut transaction_record = TransactionRecordBuilder::default().build();

        transaction_record
            .transparent_outputs
            .push(TransparentOutputBuilder::default().build());
        transaction_record
            .transparent_outputs
            .push(TransparentOutputBuilder::default().spent(spend).build());
        transaction_record.transparent_outputs.push(
            TransparentOutputBuilder::default()
                .unconfirmed_spent(spend)
                .build(),
        );
        transaction_record
            .sapling_notes
            .push(SaplingNoteBuilder::default().build());
        transaction_record
            .sapling_notes
            .push(SaplingNoteBuilder::default().spent(spend).build());
        transaction_record.sapling_notes.push(
            SaplingNoteBuilder::default()
                .unconfirmed_spent(spend)
                .build(),
        );
        transaction_record
            .orchard_notes
            .push(OrchardNoteBuilder::default().build());
        transaction_record
            .orchard_notes
            .push(OrchardNoteBuilder::default().spent(spend).build());
        transaction_record.orchard_notes.push(
            OrchardNoteBuilder::default()
                .unconfirmed_spent(spend)
                .build(),
        );

        transaction_record
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_matrix;

    use crate::wallet::notes::query::OutputQuery;

    use crate::wallet::notes::transparent::mocks::TransparentOutputBuilder;
    use crate::wallet::transaction_record::mocks::{
        nine_note_transaction_record, TransactionRecordBuilder,
    };

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
            .transparent_outputs
            .push(TransparentOutputBuilder::default().build());
        assert!(transaction_record.is_incoming_transaction());
    }

    #[test_matrix(
        [true, false],
        [true, false],
        [true, false],
        [true, false],
        [true, false],
        [true, false]
    )]
    fn query_for_ids(
        unspent: bool,
        pending_spent: bool,
        spent: bool,
        transparent: bool,
        sapling: bool,
        orchard: bool,
    ) {
        let mut valid_spend_stati = 0;
        if unspent {
            valid_spend_stati += 1;
        }
        if pending_spent {
            valid_spend_stati += 1;
        }
        if spent {
            valid_spend_stati += 1;
        }
        let mut valid_pools = 0;
        if transparent {
            valid_pools += 1;
        }
        if sapling {
            valid_pools += 1;
        }
        if orchard {
            valid_pools += 1;
        }

        let expected = valid_spend_stati * valid_pools;

        assert_eq!(
            nine_note_transaction_record()
                .query_for_ids(OutputQuery::stipulations(
                    unspent,
                    pending_spent,
                    spent,
                    transparent,
                    sapling,
                    orchard,
                ))
                .len(),
            expected,
        );
    }

    #[test_matrix(
        [true, false],
        [true, false],
        [true, false],
        [true, false],
        [true, false],
        [true, false]
    )]
    fn query_sum_value(
        unspent: bool,
        pending_spent: bool,
        spent: bool,
        transparent: bool,
        sapling: bool,
        orchard: bool,
    ) {
        let mut valid_spend_stati = 0;
        if unspent {
            valid_spend_stati += 1;
        }
        if pending_spent {
            valid_spend_stati += 1;
        }
        if spent {
            valid_spend_stati += 1;
        }
        //different pools have different mock values.
        let mut valid_pool_value = 0;
        if transparent {
            valid_pool_value += 100000;
        }
        if sapling {
            valid_pool_value += 200000;
        }
        if orchard {
            valid_pool_value += 800000;
        }

        let expected = valid_spend_stati * valid_pool_value;

        assert_eq!(
            nine_note_transaction_record().query_sum_value(OutputQuery::stipulations(
                unspent,
                pending_spent,
                spent,
                transparent,
                sapling,
                orchard,
            )),
            expected,
        );
    }

    #[test]
    fn total_value_received() {
        let transaction_record = nine_note_transaction_record();
        let old_total = transaction_record
            .pool_value_received::<orchard::note_encryption::OrchardDomain>()
            + transaction_record
                .pool_value_received::<sapling_crypto::note_encryption::SaplingDomain>()
            + transaction_record
                .transparent_outputs
                .iter()
                .map(|utxo| utxo.value)
                .sum::<u64>();
        assert_eq!(transaction_record.total_value_received(), old_total);
    }
}

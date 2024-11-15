//! An (incomplete) representation of what the Zingo instance "knows" about a transaction
//! conspicuously absent is the set of transparent inputs to the transaction.
//! by its`nature this evolves through, different states of completeness.

use crate::wallet::notes::interface::OutputConstructor;
use std::io::{self, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt as _, WriteBytesExt as _};

use incrementalmerkletree::witness::IncrementalWitness;
use orchard::tree::MerkleHashOrchard;
use zcash_client_backend::{
    wallet::NoteId,
    ShieldedProtocol::{Orchard, Sapling},
};
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use crate::wallet::traits::Recipient as _;
use crate::{
    error::ZingoLibError,
    wallet::{
        data::{OutgoingTxData, PoolNullifier, COMMITMENT_TREE_LEVELS},
        keys::unified::WalletCapability,
        notes::{
            query::OutputQuery, OrchardNote, OutputInterface, SaplingNote, ShieldedNoteInterface,
            TransparentOutput,
        },
        traits::{DomainWalletExt, ReadableWriteable as _},
    },
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

    /// Total amount of transparent funds that belong to us that were spent by this wallet in this Tx.
    pub total_transparent_value_spent: u64,

    /// Total value of all the sapling nullifiers that were spent by this wallet in this Tx
    pub total_sapling_value_spent: u64,

    /// Total value of all the orchard nullifiers that were spent by this wallet in this Tx
    pub total_orchard_value_spent: u64,

    /// All outgoing sends
    pub outgoing_tx_data: Vec<OutgoingTxData>,

    /// Price of Zec when this Tx was created
    pub price: Option<f64>,
}

// much data assignment of this struct is done through the pub fields as of january 2024. Todo: should have private fields and public methods.

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

    /// adds a note. however, does not fully commit to adding a note, because this note isnt chained into block
    pub(crate) fn add_pending_note<D: DomainWalletExt>(
        &mut self,
        note: D::Note,
        to: D::Recipient,
        output_index: usize,
    ) {
        if !D::WalletNote::get_record_outputs(self)
            .iter_mut()
            .any(|n| n.note() == &note)
        {
            let nd = D::WalletNote::from_parts(
                to.diversifier(),
                note,
                None,
                None,
                None,
                None,
                // if this is change, we'll mark it later in check_notes_mark_change
                false,
                false,
                Some(output_index as u32),
            );

            D::WalletNote::transaction_metadata_notes_mut(self).push(nd);
        }
    }

    /// returns Err(()) if note does not exist
    pub(crate) fn update_output_index<D: DomainWalletExt>(
        &mut self,
        note: D::Note,
        output_index: usize,
    ) -> Result<(), ()> {
        if let Some(n) = D::WalletNote::transaction_metadata_notes_mut(self)
            .iter_mut()
            .find(|n| n.note() == &note)
        {
            if n.output_index().is_none() {
                *n.output_index_mut() = Some(output_index as u32)
            }
            Ok(())
        } else {
            Err(())
        }
    }
}
//get
impl TransactionRecord {
    /// Get transparent outputs
    pub fn transparent_outputs(&self) -> &[TransparentOutput] {
        &self.transparent_outputs
    }

    /// Get sapling notes
    pub fn sapling_notes(&self) -> &[SaplingNote] {
        &self.sapling_notes
    }

    /// Get orchard notes
    pub fn orchard_notes(&self) -> &[OrchardNote] {
        &self.orchard_notes
    }

    /// Get sapling nullifiers
    pub fn spent_sapling_nullifiers(&self) -> &[sapling_crypto::Nullifier] {
        &self.spent_sapling_nullifiers
    }

    /// Get orchard nullifiers
    pub fn spent_orchard_nullifiers(&self) -> &[orchard::note::Nullifier] {
        &self.spent_orchard_nullifiers
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

    /// Sums all the received notes in the transaction.
    pub fn total_value_received(&self) -> u64 {
        self.query_sum_value(OutputQuery::any())
    }

    // The value that's output, but *NOT* to an explicit receiver (unless this is running on the winning validator!)
    // is the fee.
    pub(crate) fn total_value_output_to_explicit_receivers(&self) -> u64 {
        self.total_value_received() + self.value_outgoing()
    }

    /// TODO: Add Doc Comment Here!
    #[allow(deprecated)]
    #[deprecated(
        note = "replaced by `calculate_transaction_fee` method for [`crate::wallet::transaction_records_by_id::TransactionRecordsById`]"
    )]
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
        (!self.outgoing_tx_data.is_empty()) || self.total_value_output_to_explicit_receivers() != 0
    }

    /// This means there's at least one note that adds funds
    /// to this capabilities control
    #[deprecated(note = "uses unstable deprecated is_change")]
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
    fn pool_change_returned<D: DomainWalletExt>(&self) -> u64
    where
        <D as zcash_note_encryption::Domain>::Note: PartialEq + Clone,
        <D as zcash_note_encryption::Domain>::Recipient: super::traits::Recipient,
    {
        D::sum_pool_change(self)
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(note = "uses unstable deprecated functions")]
    pub fn total_change_returned(&self) -> u64 {
        self.pool_change_returned::<sapling_crypto::note_encryption::SaplingDomain>()
            + self.pool_change_returned::<orchard::note_encryption::OrchardDomain>()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(note = "replaced by total_value_input_to_transaction")]
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
            self.total_transparent_value_spent,
            self.total_sapling_value_spent,
            self.total_orchard_value_spent,
        ]
    }

    /// Gets a received note, by index and domain
    pub fn get_received_note<D: DomainWalletExt>(
        &self,
        index: u32,
    ) -> Option<
        zcash_client_backend::wallet::ReceivedNote<
            NoteId,
            <D as zcash_note_encryption::Domain>::Note,
        >,
    > {
        let note = D::WalletNote::get_record_outputs(self)
            .into_iter()
            .find(|note| *note.output_index() == Some(index));
        note.and_then(|note| {
            let txid = self.txid;
            let note_record_reference =
                NoteId::new(txid, note.to_zcb_note().protocol(), index as u16);
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

    /// get a list of unspent NoteIds with associated note values
    pub(crate) fn get_spendable_note_ids_and_values(
        &self,
        sources: &[zcash_client_backend::ShieldedProtocol],
        exclude: &[NoteId],
    ) -> Result<Vec<(NoteId, u64)>, ()> {
        let mut all = vec![];
        let mut missing_output_index = false;
        if sources.contains(&Sapling) {
            self.sapling_notes.iter().for_each(|zingo_sapling_note| {
                if zingo_sapling_note.is_unspent() {
                    if let Some(output_index) = zingo_sapling_note.output_index() {
                        let id = NoteId::new(self.txid, Sapling, *output_index as u16);
                        if !exclude.contains(&id) {
                            all.push((id, zingo_sapling_note.value()));
                        }
                    } else {
                        println!("note has no index");
                        missing_output_index = true;
                    }
                }
            });
        }
        if sources.contains(&Orchard) {
            self.orchard_notes.iter().for_each(|zingo_orchard_note| {
                if zingo_orchard_note.is_unspent() {
                    if let Some(output_index) = zingo_orchard_note.output_index() {
                        let id = NoteId::new(self.txid, Orchard, *output_index as u16);
                        if !exclude.contains(&id) {
                            all.push((id, zingo_orchard_note.value()));
                        }
                    } else {
                        println!("note has no index");
                        missing_output_index = true;
                    }
                }
            });
        }
        if missing_output_index {
            Err(())
        } else {
            Ok(all)
        }
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

        let pending = if version <= 20 {
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

        let total_transparent_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_sapling_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_orchard_value_spent = if version >= 22 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };

        let outgoing_metadata = match version {
            ..24 => zcash_encoding::Vector::read(&mut reader, |r| OutgoingTxData::read_old(r))?,
            24.. => zcash_encoding::Vector::read(&mut reader, |r| OutgoingTxData::read(r))?,
        };
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
        let status = zingo_status::confirmation_status::ConfirmationStatus::from_blockheight_and_pending_bool(block, pending);
        Ok(Self {
            status,
            datetime,
            txid: transaction_id,
            sapling_notes,
            orchard_notes,
            transparent_outputs: utxos,
            spent_sapling_nullifiers,
            spent_orchard_nullifiers,
            total_transparent_value_spent,
            total_sapling_value_spent,
            total_orchard_value_spent,
            outgoing_tx_data: outgoing_metadata,
            price: zec_price,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn serialized_version() -> u64 {
        24
    }

    /// TODO: Add Doc Comment Here!
    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        let block: u32 = self.status.get_height().into();
        writer.write_i32::<LittleEndian>(block as i32)?;

        writer.write_u8(if !self.status.is_confirmed() { 1 } else { 0 })?;

        writer.write_u64::<LittleEndian>(self.datetime)?;

        writer.write_all(self.txid.as_ref())?;

        zcash_encoding::Vector::write(&mut writer, &self.sapling_notes, |w, nd| nd.write(w, ()))?;
        zcash_encoding::Vector::write(&mut writer, &self.orchard_notes, |w, nd| nd.write(w, ()))?;
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

/// TODO: doc comment
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TransactionKind {
    /// TODO: doc comment
    Sent(SendType),
    /// TODO: doc comment
    Received,
}

impl std::fmt::Display for TransactionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TransactionKind::Received => write!(f, "received"),
            TransactionKind::Sent(SendType::Send) => write!(f, "sent"),
            TransactionKind::Sent(SendType::Shield) => write!(f, "shield"),
            TransactionKind::Sent(SendType::SendToSelf) => write!(f, "send-to-self"),
        }
    }
}

/// TODO: doc comment
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SendType {
    /// Transaction is sending funds to recipient other than the creator
    Send,
    /// Transaction is only sending funds from transparent pool to the creator's shielded pool
    Shield,
    /// Transaction is only sending funds to the creator's address(es) and is not a shield
    SendToSelf,
}

#[cfg(any(test, feature = "test-elevation"))]
pub mod mocks {
    //! Mock version of the struct for testing
    use zcash_primitives::transaction::TxId;

    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingo_status::confirmation_status::ConfirmationStatus::Confirmed;
    use zingo_status::confirmation_status::ConfirmationStatus::Mempool;

    use crate::{
        mocks::{
            nullifier::{OrchardNullifierBuilder, SaplingNullifierBuilder},
            random_txid,
        },
        utils::{build_method, build_method_push, build_push_list},
        wallet::{
            data::mocks::OutgoingTxDataBuilder,
            notes::{
                orchard::mocks::OrchardNoteBuilder, sapling::mocks::SaplingNoteBuilder,
                transparent::mocks::TransparentOutputBuilder,
            },
        },
    };

    use super::TransactionRecord;

    /// to create a mock TransactionRecord
    pub(crate) struct TransactionRecordBuilder {
        status: Option<ConfirmationStatus>,
        datetime: Option<u64>,
        txid: Option<TxId>,
        spent_sapling_nullifiers: Vec<SaplingNullifierBuilder>,
        spent_orchard_nullifiers: Vec<OrchardNullifierBuilder>,
        transparent_outputs: Vec<TransparentOutputBuilder>,
        sapling_notes: Vec<SaplingNoteBuilder>,
        orchard_notes: Vec<OrchardNoteBuilder>,
        total_transparent_value_spent: Option<u64>,
        outgoing_tx_data: Vec<OutgoingTxDataBuilder>,
    }
    #[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
    impl TransactionRecordBuilder {
        /// blank builder
        pub fn new() -> Self {
            Self {
                status: None,
                datetime: None,
                txid: None,
                spent_sapling_nullifiers: vec![],
                spent_orchard_nullifiers: vec![],
                transparent_outputs: vec![],
                sapling_notes: vec![],
                orchard_notes: vec![],
                total_transparent_value_spent: None,
                outgoing_tx_data: vec![],
            }
        }
        // Methods to set each field
        build_method!(status, ConfirmationStatus);
        build_method!(datetime, u64);
        build_method!(txid, TxId);
        build_method_push!(spent_sapling_nullifiers, SaplingNullifierBuilder);
        build_method_push!(spent_orchard_nullifiers, OrchardNullifierBuilder);
        build_method_push!(transparent_outputs, TransparentOutputBuilder);
        build_method_push!(sapling_notes, SaplingNoteBuilder);
        build_method_push!(orchard_notes, OrchardNoteBuilder);
        build_method!(total_transparent_value_spent, u64);
        build_method_push!(outgoing_tx_data, OutgoingTxDataBuilder);

        /// Use the mockery of random_txid to get one?
        pub fn randomize_txid(&mut self) -> &mut Self {
            self.txid(crate::mocks::random_txid())
        }

        /// Sets the output indexes of all contained notes
        pub fn set_output_indexes(&mut self) -> &mut Self {
            for (i, toutput) in self.transparent_outputs.iter_mut().enumerate() {
                toutput.output_index = Some(i as u64);
            }
            for (i, snote) in self.sapling_notes.iter_mut().enumerate() {
                snote.output_index = Some(Some(i as u32));
            }
            for (i, snote) in self.orchard_notes.iter_mut().enumerate() {
                snote.output_index = Some(Some(i as u32));
            }
            self
        }

        /// builds a mock TransactionRecord after all pieces are supplied
        pub fn build(&self) -> TransactionRecord {
            let mut transaction_record = TransactionRecord::new(
                self.status.unwrap(),
                self.datetime.unwrap(),
                &self.txid.unwrap(),
            );
            build_push_list!(spent_sapling_nullifiers, self, transaction_record);
            build_push_list!(spent_orchard_nullifiers, self, transaction_record);
            build_push_list!(transparent_outputs, self, transaction_record);
            build_push_list!(sapling_notes, self, transaction_record);
            build_push_list!(orchard_notes, self, transaction_record);
            build_push_list!(outgoing_tx_data, self, transaction_record);
            transaction_record.total_transparent_value_spent =
                self.total_transparent_value_spent.unwrap();
            transaction_record
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
                txid: Some(crate::mocks::random_txid()),
                spent_sapling_nullifiers: vec![],
                spent_orchard_nullifiers: vec![],
                transparent_outputs: vec![],
                sapling_notes: vec![],
                orchard_notes: vec![],
                total_transparent_value_spent: Some(0),
                outgoing_tx_data: vec![],
            }
        }
    }

    /// creates a TransactionRecord holding each type of note with custom values.
    #[allow(clippy::too_many_arguments)]
    pub fn nine_note_transaction_record(
        transparent_unspent: u64,
        transparent_spent: u64,
        transparent_semi_spent: u64,
        sapling_unspent: u64,
        sapling_spent: u64,
        sapling_semi_spent: u64,
        orchard_unspent: u64,
        orchard_spent: u64,
        orchard_semi_spent: u64,
    ) -> TransactionRecord {
        let spend = Some((random_txid(), Confirmed(112358.into())));
        let pending_spend = Some((random_txid(), Mempool(853211.into())));

        TransactionRecordBuilder::default()
            .transparent_outputs(
                TransparentOutputBuilder::default()
                    .value(transparent_unspent)
                    .clone(),
            )
            .transparent_outputs(
                TransparentOutputBuilder::default()
                    .spending_tx_status(spend)
                    .value(transparent_spent)
                    .clone(),
            )
            .transparent_outputs(
                TransparentOutputBuilder::default()
                    .spending_tx_status(pending_spend)
                    .value(transparent_semi_spent)
                    .clone(),
            )
            .sapling_notes(SaplingNoteBuilder::default().value(sapling_unspent).clone())
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .spending_tx_status(spend)
                    .value(sapling_spent)
                    .clone(),
            )
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .spending_tx_status(pending_spend)
                    .value(sapling_semi_spent)
                    .clone(),
            )
            .orchard_notes(OrchardNoteBuilder::default().value(orchard_unspent).clone())
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .spending_tx_status(spend)
                    .value(orchard_spent)
                    .clone(),
            )
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .spending_tx_status(pending_spend)
                    .value(orchard_semi_spent)
                    .clone(),
            )
            .randomize_txid()
            .set_output_indexes()
            .build()
    }

    /// default values are multiples of 10_000
    pub fn nine_note_transaction_record_default() -> TransactionRecord {
        nine_note_transaction_record(
            10_000, 20_000, 30_000, 40_000, 50_000, 60_000, 70_000, 80_000, 90_000,
        )
    }
    #[test]
    fn check_nullifier_indices() {
        let sap_null_one = SaplingNullifierBuilder::new()
            .assign_unique_nullifier()
            .clone();
        let sap_null_two = SaplingNullifierBuilder::new()
            .assign_unique_nullifier()
            .clone();
        let orch_null_one = OrchardNullifierBuilder::new()
            .assign_unique_nullifier()
            .clone();
        let orch_null_two = OrchardNullifierBuilder::new()
            .assign_unique_nullifier()
            .clone();
        let sent_transaction_record = TransactionRecordBuilder::default()
            .status(ConfirmationStatus::Confirmed(15.into()))
            .spent_sapling_nullifiers(sap_null_one.clone())
            .spent_sapling_nullifiers(sap_null_two.clone())
            .spent_orchard_nullifiers(orch_null_one.clone())
            .spent_orchard_nullifiers(orch_null_two.clone())
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default())
            .orchard_notes(OrchardNoteBuilder::default())
            .outgoing_tx_data(OutgoingTxDataBuilder::default())
            .build();
        assert_eq!(
            sent_transaction_record.spent_sapling_nullifiers[0],
            sap_null_one.build()
        );
        assert_eq!(
            sent_transaction_record.spent_sapling_nullifiers[1],
            sap_null_two.build()
        );
        assert_eq!(
            sent_transaction_record.spent_orchard_nullifiers[0],
            orch_null_one.build()
        );
        assert_eq!(
            sent_transaction_record.spent_orchard_nullifiers[1],
            orch_null_two.build()
        );
    }
}

#[cfg(test)]
mod tests {
    //use proptest::prelude::proptest;
    use test_case::test_matrix;

    use sapling_crypto::note_encryption::SaplingDomain;
    use zcash_client_backend::wallet::NoteId;
    use zcash_client_backend::ShieldedProtocol::{Orchard, Sapling};

    use crate::wallet::notes::{
        query::{OutputPoolQuery, OutputQuery, OutputSpendStatusQuery},
        Output, OutputInterface,
    };
    use crate::wallet::transaction_record::mocks::{
        nine_note_transaction_record, nine_note_transaction_record_default,
        TransactionRecordBuilder,
    };

    #[test]
    pub fn blank_record() {
        let new = TransactionRecordBuilder::default().build();
        assert_eq!(new.total_transparent_value_spent, 0);
        assert_eq!(
            new.pool_change_returned::<orchard::note_encryption::OrchardDomain>(),
            0
        );
        assert_eq!(
            new.pool_change_returned::<sapling_crypto::note_encryption::SaplingDomain>(),
            0
        );
        assert_eq!(new.total_value_received(), 0);
        assert_eq!(new.value_outgoing(), 0);
        let t: [u64; 3] = [0, 0, 0];
        assert_eq!(new.value_spent_by_pool(), t);
    }

    #[test_matrix(
        [true, false],
        [true, false],
        [true, false],
        [true, false],
        [true, false],
        [true, false]
    )]
    fn query_for_outputs(
        unspent: bool,
        pending_spent: bool,
        spent: bool,
        transparent: bool,
        sapling: bool,
        orchard: bool,
    ) {
        let queried_spend_state = OutputSpendStatusQuery {
            unspent,
            pending_spent,
            spent,
        };
        let queried_pools = OutputPoolQuery {
            transparent,
            sapling,
            orchard,
        };
        let mut queried_spend_state_count = 0;
        if unspent {
            queried_spend_state_count += 1;
        }
        if pending_spent {
            queried_spend_state_count += 1;
        }
        if spent {
            queried_spend_state_count += 1;
        }
        let mut queried_pools_count = 0;
        if transparent {
            queried_pools_count += 1;
        }
        if sapling {
            queried_pools_count += 1;
        }
        if orchard {
            queried_pools_count += 1;
        }

        let expected = queried_spend_state_count * queried_pools_count;

        let default_nn_transaction_record = nine_note_transaction_record_default();
        let requested_outputs: Vec<Output> =
            Output::get_record_outputs(&default_nn_transaction_record)
                .iter()
                .filter(|o| o.spend_status_query(queried_spend_state))
                .filter(|&o| o.pool_query(queried_pools))
                .cloned()
                .collect();
        assert_eq!(requested_outputs.len(), expected);
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
        let mut valid_spend_state = 0;
        if unspent {
            valid_spend_state += 1;
        }
        if pending_spent {
            valid_spend_state += 1;
        }
        if spent {
            valid_spend_state += 1;
        }
        //different pools have different mock values.
        let mut valid_pool_value = 0;
        if transparent {
            valid_pool_value += 100_000;
        }
        if sapling {
            valid_pool_value += 200_000;
        }
        if orchard {
            valid_pool_value += 800_000;
        }

        let expected = valid_spend_state * valid_pool_value;

        assert_eq!(
            nine_note_transaction_record(
                100_000, 100_000, 100_000, 200_000, 200_000, 200_000, 800_000, 800_000, 800_000
            )
            .query_sum_value(OutputQuery::stipulations(
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
    fn select_spendable_note_ids_and_values() {
        let transaction_record = nine_note_transaction_record_default();

        let unspent_ids_and_values = transaction_record
            .get_spendable_note_ids_and_values(&[Sapling, Orchard], &[])
            .unwrap();

        assert_eq!(
            unspent_ids_and_values,
            vec![
                (NoteId::new(transaction_record.txid, Sapling, 0), 40_000),
                (NoteId::new(transaction_record.txid, Orchard, 0), 70_000)
            ]
        );
    }

    #[test]
    fn get_received_note() {
        let transaction_record = nine_note_transaction_record(
            100_000_000,
            200_000_000,
            400_000_000,
            100_000_000,
            200_000_000,
            400_000_000,
            100_000_000,
            200_000_000,
            400_000_000,
        );

        for (i, value) in transaction_record
            .sapling_notes
            .iter()
            .map(|note| note.sapling_crypto_note.value())
            .enumerate()
        {
            assert_eq!(
                transaction_record
                    .get_received_note::<SaplingDomain>(i as u32)
                    .unwrap()
                    .note_value()
                    .unwrap()
                    .into_u64(),
                value.inner()
            )
        }
    }
}

use incrementalmerkletree::witness::IncrementalWitness;
use zcash_primitives::{sapling, transaction::TxId};

use crate::error::ZingoLibError;
use crate::wallet::notes;

use super::{
    data::{OutgoingTxData, PoolNullifier},
    *,
};

///  Everything (SOMETHING) about a transaction
#[derive(Debug)]
pub struct TransactionRecord {
    // the relationship of the transaction to the blockchain. can be either Broadcast (to mempool}, or Confirmed.
    pub status: ConfirmationStatus,

    // Timestamp of Tx. Added in v4
    pub datetime: u64,

    // Txid of this transaction. It's duplicated here (It is also the Key in the HashMap that points to this
    // WalletTx in LightWallet::txs)
    pub txid: TxId,

    // List of all nullifiers spent by this wallet in this Tx.
    pub spent_sapling_nullifiers: Vec<zcash_primitives::sapling::Nullifier>,

    // List of all nullifiers spent by this wallet in this Tx. These nullifiers belong to the wallet.
    pub spent_orchard_nullifiers: Vec<orchard::note::Nullifier>,

    // List of all sapling notes received by this wallet in this tx. Some of these might be change notes.
    pub sapling_notes: Vec<notes::SaplingNote>,

    // List of all sapling notes received by this wallet in this tx. Some of these might be change notes.
    pub orchard_notes: Vec<notes::OrchardNote>,

    // List of all Utxos by this wallet received in this Tx. Some of these might be change notes
    pub transparent_notes: Vec<notes::TransparentNote>,

    // Total value of all the sapling nullifiers that were spent by this wallet in this Tx
    pub total_sapling_value_spent: u64,

    // Total value of all the orchard nullifiers that were spent by this wallet in this Tx
    pub total_orchard_value_spent: u64,

    // Total amount of transparent funds that belong to us that were spent by this wallet in this Tx.
    pub total_transparent_value_spent: u64,

    // All outgoing sends
    pub outgoing_tx_data: Vec<OutgoingTxData>,

    // Price of Zec when this Tx was created
    pub price: Option<f64>,
}

// set
impl TransactionRecord {
    pub fn new(status: ConfirmationStatus, datetime: u64, transaction_id: &TxId) -> Self {
        TransactionRecord {
            status,
            datetime,
            txid: *transaction_id,
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
#[cfg(test)]
mod get_transaction_fee_tests {
    use super::*;
    #[test]
    fn get_transaction_fee_negative_control() {
        let txid = TxId::from_bytes([0u8; 32]);
        let transaction_height = 10u32;
        let transaction_block_height = BlockHeight::from_u32(transaction_height);
        let status = ConfirmationStatus::Confirmed(transaction_block_height);
        let transaction_record = TransactionRecord::new(status, 1705517102, &txid);
        assert_eq!(transaction_record.get_transaction_fee().unwrap(), 0);
    }
    #[test]
    fn get_transaction_fee_transparent_receipts() {
        use crate::test_framework::TransparentNoteBuilder;
        let transaction_height = 10u32;
        let transaction_block_height = BlockHeight::from_u32(transaction_height);
        let txid = TxId::from_bytes([0u8; 32]);
        let transparent_note_one = TransparentNoteBuilder::new()
            .value(10)
            .spent(Some(txid))
            .build();
        let transparent_note_two = TransparentNoteBuilder::new()
            .value(20)
            .spent(Some(txid))
            .build();
        let transparent_note_three = TransparentNoteBuilder::new()
            .value(30)
            .spent(Some(txid))
            .build();
        let status = ConfirmationStatus::Confirmed(transaction_block_height);
        let mut transaction_record = TransactionRecord::new(status, 1705517102, &txid);
        transaction_record
            .transparent_notes
            .push(transparent_note_one);
        transaction_record
            .transparent_notes
            .push(transparent_note_two);
        transaction_record
            .transparent_notes
            .push(transparent_note_three);
        assert_eq!(transaction_record.get_transaction_fee().unwrap(), 0);
    }
}
impl TransactionRecord {
    pub fn get_transparent_value_spent(&self) -> u64 {
        self.total_transparent_value_spent
    }
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

    // TODO: This is incorrect in the edge case where where we have a send-to-self with
    // no text memo and 0-value fee
    pub fn is_outgoing_transaction(&self) -> bool {
        (!self.outgoing_tx_data.is_empty()) || self.total_value_spent() != 0
    }
    pub fn is_incoming_transaction(&self) -> bool {
        self.sapling_notes.iter().any(|note| !note.is_change())
            || self.orchard_notes.iter().any(|note| !note.is_change())
            || !self.transparent_notes.is_empty()
    }
    pub fn net_spent(&self) -> u64 {
        assert!(self.is_outgoing_transaction());
        self.total_value_spent() - self.total_change_returned()
    }
    fn pool_change_returned<D: DomainWalletExt>(&self) -> u64
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        D::sum_pool_change(self)
    }

    pub fn pool_value_received<D: DomainWalletExt>(&self) -> u64
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        D::to_notes_vec(self)
            .iter()
            .map(|note_and_metadata| note_and_metadata.value())
            .sum()
    }
    pub fn total_change_returned(&self) -> u64 {
        self.pool_change_returned::<SaplingDomain<ChainType>>()
            + self.pool_change_returned::<OrchardDomain>()
    }
    pub fn total_value_received(&self) -> u64 {
        self.pool_value_received::<OrchardDomain>()
            + self.pool_value_received::<SaplingDomain<ChainType>>()
            + self
                .transparent_notes
                .iter()
                .map(|utxo| utxo.value)
                .sum::<u64>()
    }
    pub fn total_value_spent(&self) -> u64 {
        self.value_spent_by_pool().iter().sum()
    }

    pub fn value_outgoing(&self) -> u64 {
        self.outgoing_tx_data
            .iter()
            .fold(0, |running_total, tx_data| tx_data.value + running_total)
    }

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
    #[allow(clippy::type_complexity)]
    pub fn read<R: Read>(
        mut reader: R,
        (wallet_capability, mut trees): (
            &WalletCapability,
            Option<&mut (
                Vec<(
                    IncrementalWitness<sapling::Node, COMMITMENT_TREE_LEVELS>,
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

        let sapling_notes = Vector::read_collected_mut(&mut reader, |r| {
            notes::SaplingNote::read(r, (wallet_capability, trees.as_mut().map(|t| &mut t.0)))
        })?;
        let orchard_notes = if version > 22 {
            Vector::read_collected_mut(&mut reader, |r| {
                notes::OrchardNote::read(r, (wallet_capability, trees.as_mut().map(|t| &mut t.1)))
            })?
        } else {
            vec![]
        };
        let utxos = Vector::read(&mut reader, |r| notes::TransparentNote::read(r))?;

        let total_sapling_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_transparent_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_orchard_value_spent = if version >= 22 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };

        // Outgoing metadata was only added in version 2
        let outgoing_metadata = Vector::read(&mut reader, |r| OutgoingTxData::read(r))?;

        let _full_tx_scanned = reader.read_u8()? > 0;

        let zec_price = if version <= 4 {
            None
        } else {
            Optional::read(&mut reader, |r| r.read_f64::<LittleEndian>())?
        };

        let spent_sapling_nullifiers = if version <= 5 {
            vec![]
        } else {
            Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(zcash_primitives::sapling::Nullifier(n))
            })?
        };

        let spent_orchard_nullifiers = if version <= 21 {
            vec![]
        } else {
            Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(orchard::note::Nullifier::from_bytes(&n).unwrap())
            })?
        };
        let status = ConfirmationStatus::from_blockheight_and_unconfirmed_bool(block, unconfirmed);
        Ok(Self {
            status,
            datetime,
            txid: transaction_id,
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

    pub fn serialized_version() -> u64 {
        23
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        let block: u32 = self.status.get_height().into();
        writer.write_i32::<LittleEndian>(block as i32)?;

        writer.write_u8(if !self.status.is_confirmed() { 1 } else { 0 })?;

        writer.write_u64::<LittleEndian>(self.datetime)?;

        writer.write_all(self.txid.as_ref())?;

        Vector::write(&mut writer, &self.sapling_notes, |w, nd| nd.write(w))?;
        Vector::write(&mut writer, &self.orchard_notes, |w, nd| nd.write(w))?;
        Vector::write(&mut writer, &self.transparent_notes, |w, u| u.write(w))?;

        for pool in self.value_spent_by_pool() {
            writer.write_u64::<LittleEndian>(pool)?;
        }

        // Write the outgoing metadata
        Vector::write(&mut writer, &self.outgoing_tx_data, |w, om| om.write(w))?;

        writer.write_u8(0)?;

        Optional::write(&mut writer, self.price, |w, p| {
            w.write_f64::<LittleEndian>(p)
        })?;

        Vector::write(&mut writer, &self.spent_sapling_nullifiers, |w, n| {
            w.write_all(&n.0)
        })?;
        Vector::write(&mut writer, &self.spent_orchard_nullifiers, |w, n| {
            w.write_all(&n.to_bytes())
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::wallet::utils::txid_from_slice;

    use super::*;

    #[test]
    pub fn blank_record() {
        let new = TransactionRecord::new(
            ConfirmationStatus::Confirmed(2_000_000.into()),
            103,
            &txid_from_slice(&[0u8; 32]),
        );
        assert_eq!(new.get_transparent_value_spent(), 0);
        assert_eq!(new.get_transaction_fee().unwrap(), 0);
        assert_eq!(new.is_outgoing_transaction(), false);
        assert_eq!(new.is_incoming_transaction(), false);
        // assert_eq!(new.net_spent(), 0);
        assert_eq!(new.pool_change_returned::<OrchardDomain>(), 0);
        assert_eq!(new.pool_change_returned::<SaplingDomain<ChainType>>(), 0);
        assert_eq!(new.total_value_received(), 0);
        assert_eq!(new.total_value_spent(), 0);
        assert_eq!(new.value_outgoing(), 0);
        let t: [u64; 3] = [0, 0, 0];
        assert_eq!(new.value_spent_by_pool(), t);
    }
}

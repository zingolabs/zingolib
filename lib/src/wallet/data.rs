use crate::blaze::fixed_size_buffer::FixedSizeBuffer;
use crate::compact_formats::CompactBlock;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use orchard::{
    keys::{Diversifier as OrchardDiversifier, SpendingKey as OrchardSpendingKey},
    note::{Note as OrchardNote, Nullifier as OrchardNullifier},
    tree::MerkleHashOrchard,
};
use prost::Message;
use std::convert::TryFrom;
use std::io::{self, Read, Write};
use std::usize;
use zcash_encoding::{Optional, Vector};
use zcash_primitives::{consensus::BlockHeight, zip32::ExtendedSpendingKey};
use zcash_primitives::{
    memo::Memo,
    merkle_tree::{CommitmentTree, IncrementalWitness},
    sapling::{
        Diversifier as SaplingDiversifier, Node as SaplingNode, Note as SaplingNote,
        Nullifier as SaplingNullifier, Rseed,
    },
    transaction::{components::OutPoint, TxId},
    zip32::ExtendedFullViewingKey,
};
use zcash_primitives::{memo::MemoBytes, merkle_tree::Hashable};

use super::traits::ReadableWriteable;

/// This type is motivated by the IPC architecture where (currently) channels traffic in
/// `(TxId, WalletNullifier, BlockHeight, Option<u32>)`.  This enum permits a single channel
/// type to handle nullifiers from different domains.
/// <https://github.com/zingolabs/zingolib/issues/64>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolNullifier {
    Sapling(SaplingNullifier),
    Orchard(OrchardNullifier),
}

impl std::hash::Hash for PoolNullifier {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            PoolNullifier::Sapling(n) => {
                state.write_u8(0);
                n.0.hash(state);
            }
            PoolNullifier::Orchard(n) => {
                state.write_u8(1);
                n.to_bytes().hash(state);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockData {
    pub(crate) ecb: Vec<u8>,
    pub height: u64,
}

impl BlockData {
    pub fn serialized_version() -> u64 {
        return 20;
    }

    pub(crate) fn new_with(height: u64, hash: &str) -> Self {
        let mut cb = CompactBlock::default();
        cb.hash = hex::decode(hash)
            .unwrap()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();

        let mut ecb = vec![];
        cb.encode(&mut ecb).unwrap();

        Self { ecb, height }
    }

    pub(crate) fn new(mut cb: CompactBlock) -> Self {
        for compact_transaction in &mut cb.vtx {
            for co in &mut compact_transaction.outputs {
                co.ciphertext.clear();
                co.epk.clear();
            }
        }

        cb.header.clear();
        let height = cb.height;

        let mut ecb = vec![];
        cb.encode(&mut ecb).unwrap();

        Self { ecb, height }
    }

    pub(crate) fn cb(&self) -> CompactBlock {
        let b = self.ecb.clone();
        CompactBlock::decode(&b[..]).unwrap()
    }

    pub(crate) fn hash(&self) -> String {
        self.cb().hash().to_string()
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let height = reader.read_i32::<LittleEndian>()? as u64;

        let mut hash_bytes = [0; 32];
        reader.read_exact(&mut hash_bytes)?;
        hash_bytes.reverse();
        let hash = hex::encode(hash_bytes);

        // We don't need this, but because of a quirk, the version is stored later, so we can't actually
        // detect the version here. So we write an empty tree and read it back here
        let tree = CommitmentTree::<SaplingNode>::read(&mut reader)?;
        let _tree = if tree.size() == 0 { None } else { Some(tree) };

        let version = reader.read_u64::<LittleEndian>()?;

        let ecb = if version <= 11 {
            vec![]
        } else {
            Vector::read(&mut reader, |r| r.read_u8())?
        };

        if ecb.is_empty() {
            Ok(BlockData::new_with(height, hash.as_str()))
        } else {
            Ok(BlockData { ecb, height })
        }
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_i32::<LittleEndian>(self.height as i32)?;

        let hash_bytes: Vec<_> = hex::decode(self.hash())
            .unwrap()
            .into_iter()
            .rev()
            .collect();
        writer.write_all(&hash_bytes[..])?;

        CommitmentTree::<SaplingNode>::empty().write(&mut writer)?;
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // Write the ecb as well
        Vector::write(&mut writer, &self.ecb, |w, b| w.write_u8(*b))?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct WitnessCache<Node: Hashable> {
    pub(crate) witnesses: Vec<IncrementalWitness<Node>>,
    pub(crate) top_height: u64,
}

impl<Node: Hashable> WitnessCache<Node> {
    pub fn new(witnesses: Vec<IncrementalWitness<Node>>, top_height: u64) -> Self {
        Self {
            witnesses,
            top_height,
        }
    }

    pub fn empty() -> Self {
        Self {
            witnesses: vec![],
            top_height: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.witnesses.len()
    }

    pub fn is_empty(&self) -> bool {
        self.witnesses.is_empty()
    }

    pub fn clear(&mut self) {
        self.witnesses.clear();
    }

    pub fn get(&self, i: usize) -> Option<&IncrementalWitness<Node>> {
        self.witnesses.get(i)
    }

    #[cfg(test)]
    pub fn get_from_last(&self, i: usize) -> Option<&IncrementalWitness<Node>> {
        self.witnesses.get(self.len() - i - 1)
    }

    pub fn last(&self) -> Option<&IncrementalWitness<Node>> {
        self.witnesses.last()
    }

    pub(crate) fn into_fsb(self, fsb: &mut FixedSizeBuffer<IncrementalWitness<Node>>) {
        self.witnesses.into_iter().for_each(|w| fsb.push(w));
    }

    pub fn pop(&mut self, at_height: u64) {
        while !self.witnesses.is_empty() && self.top_height >= at_height {
            self.witnesses.pop();
            self.top_height -= 1;
        }
    }

    // pub fn get_as_string(&self, i: usize) -> String {
    //     if i >= self.witnesses.len() {
    //         return "".to_string();
    //     }

    //     let mut buf = vec![];
    //     self.get(i).unwrap().write(&mut buf).unwrap();
    //     return hex::encode(buf);
    // }
}
pub struct ReceivedSaplingNoteAndMetadata {
    // Technically, this should be recoverable from the account number,
    // but we're going to refactor this in the future, so I'll write it again here.
    pub(super) extfvk: ExtendedFullViewingKey,

    pub diversifier: SaplingDiversifier,
    pub note: SaplingNote,

    // Witnesses for the last 100 blocks. witnesses.last() is the latest witness
    pub(crate) witnesses: WitnessCache<SaplingNode>,
    pub(super) nullifier: SaplingNullifier,
    pub spent: Option<(TxId, u32)>, // If this note was confirmed spent

    // If this note was spent in a send, but has not yet been confirmed.
    // Contains the transaction id and height at which it was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
    pub memo: Option<Memo>,
    pub is_change: bool,

    // If the spending key is available in the wallet (i.e., whether to keep witness up-to-date)
    pub have_spending_key: bool,
}

pub struct ReceivedOrchardNoteAndMetadata {
    pub(super) fvk: orchard::keys::FullViewingKey,

    pub diversifier: OrchardDiversifier,
    pub note: OrchardNote,

    // Witnesses for the last 100 blocks. witnesses.last() is the latest witness
    pub(crate) witnesses: WitnessCache<MerkleHashOrchard>,
    pub(super) nullifier: OrchardNullifier,
    pub spent: Option<(TxId, u32)>, // If this note was confirmed spent

    // If this note was spent in a send, but has not yet been confirmed.
    // Contains the transaction id and height at which it was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
    pub memo: Option<Memo>,
    pub is_change: bool,

    // If the spending key is available in the wallet (i.e., whether to keep witness up-to-date)
    pub have_spending_key: bool,
}

impl std::fmt::Debug for ReceivedSaplingNoteAndMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaplingNoteData")
            .field("extfvk", &self.extfvk)
            .field("diversifier", &self.diversifier)
            .field("note", &self.note)
            .field("nullifier", &self.nullifier)
            .field("spent", &self.spent)
            .field("unconfirmed_spent", &self.unconfirmed_spent)
            .field("memo", &self.memo)
            .field("extfvk", &self.extfvk)
            .field("diversifier", &self.diversifier)
            .field("note", &self.note)
            .field("nullifier", &self.nullifier)
            .field("spent", &self.spent)
            .field("unconfirmed_spent", &self.unconfirmed_spent)
            .field("memo", &self.memo)
            .field("is_change", &self.is_change)
            .finish_non_exhaustive()
    }
}

// Reading a note also needs the corresponding address to read from.
pub(crate) fn read_sapling_rseed<R: Read>(mut reader: R) -> io::Result<Rseed> {
    let note_type = reader.read_u8()?;

    let mut r_bytes: [u8; 32] = [0; 32];
    reader.read_exact(&mut r_bytes)?;

    let r = match note_type {
        1 => Rseed::BeforeZip212(jubjub::Fr::from_bytes(&r_bytes).unwrap()),
        2 => Rseed::AfterZip212(r_bytes),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "Bad note type")),
    };

    Ok(r)
}

pub(crate) fn write_sapling_rseed<W: Write>(mut writer: W, rseed: &Rseed) -> io::Result<()> {
    let note_type = match rseed {
        Rseed::BeforeZip212(_) => 1,
        Rseed::AfterZip212(_) => 2,
    };
    writer.write_u8(note_type)?;

    match rseed {
        Rseed::BeforeZip212(fr) => writer.write_all(&fr.to_bytes()),
        Rseed::AfterZip212(b) => writer.write_all(b),
    }
}

#[derive(Clone, Debug)]
pub struct Utxo {
    pub address: String,
    pub txid: TxId,
    pub output_index: u64,
    pub script: Vec<u8>,
    pub value: u64,
    pub height: i32,

    pub spent_at_height: Option<i32>,
    pub spent: Option<TxId>, // If this utxo was confirmed spent

    // If this utxo was spent in a send, but has not yet been confirmed.
    // Contains the txid and height at which the Tx was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
}

impl Utxo {
    pub fn serialized_version() -> u64 {
        return 3;
    }

    pub fn to_outpoint(&self) -> OutPoint {
        OutPoint::new(*self.txid.as_ref(), self.output_index as u32)
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;

        let address_len = reader.read_i32::<LittleEndian>()?;
        let mut address_bytes = vec![0; address_len as usize];
        reader.read_exact(&mut address_bytes)?;
        let address = String::from_utf8(address_bytes).unwrap();
        assert_eq!(address.chars().take(1).collect::<Vec<char>>()[0], 't');

        let mut transaction_id_bytes = [0; 32];
        reader.read_exact(&mut transaction_id_bytes)?;
        let transaction_id = TxId::from_bytes(transaction_id_bytes);

        let output_index = reader.read_u64::<LittleEndian>()?;
        let value = reader.read_u64::<LittleEndian>()?;
        let height = reader.read_i32::<LittleEndian>()?;

        let script = Vector::read(&mut reader, |r| {
            let mut byte = [0; 1];
            r.read_exact(&mut byte)?;
            Ok(byte[0])
        })?;

        let spent = Optional::read(&mut reader, |r| {
            let mut transaction_bytes = [0u8; 32];
            r.read_exact(&mut transaction_bytes)?;
            Ok(TxId::from_bytes(transaction_bytes))
        })?;

        let spent_at_height = if version <= 1 {
            None
        } else {
            Optional::read(&mut reader, |r| r.read_i32::<LittleEndian>())?
        };

        let unconfirmed_spent = if version <= 2 {
            None
        } else {
            Optional::read(&mut reader, |r| {
                let mut transaction_bytes = [0u8; 32];
                r.read_exact(&mut transaction_bytes)?;

                let height = r.read_u32::<LittleEndian>()?;
                Ok((TxId::from_bytes(transaction_bytes), height))
            })?
        };

        Ok(Utxo {
            address,
            txid: transaction_id,
            output_index,
            script,
            value,
            height,
            spent_at_height,
            spent,
            unconfirmed_spent,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        writer.write_u32::<LittleEndian>(self.address.as_bytes().len() as u32)?;
        writer.write_all(self.address.as_bytes())?;

        writer.write_all(self.txid.as_ref())?;

        writer.write_u64::<LittleEndian>(self.output_index)?;
        writer.write_u64::<LittleEndian>(self.value)?;
        writer.write_i32::<LittleEndian>(self.height)?;

        Vector::write(&mut writer, &self.script, |w, b| w.write_all(&[*b]))?;

        Optional::write(&mut writer, self.spent, |w, transaction_id| {
            w.write_all(transaction_id.as_ref())
        })?;

        Optional::write(&mut writer, self.spent_at_height, |w, s| {
            w.write_i32::<LittleEndian>(s)
        })?;

        Optional::write(
            &mut writer,
            self.unconfirmed_spent,
            |w, (transaction_id, height)| {
                w.write_all(transaction_id.as_ref())?;
                w.write_u32::<LittleEndian>(height)
            },
        )?;

        Ok(())
    }
}

pub struct OutgoingTxMetadata {
    pub to_address: String,
    pub value: u64,
    pub memo: Memo,
    pub ua: Option<String>,
}

impl PartialEq for OutgoingTxMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.to_address == other.to_address && self.value == other.value && self.memo == other.memo
    }
}

impl OutgoingTxMetadata {
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let address_len = reader.read_u64::<LittleEndian>()?;
        let mut address_bytes = vec![0; address_len as usize];
        reader.read_exact(&mut address_bytes)?;
        let address = String::from_utf8(address_bytes).unwrap();

        let value = reader.read_u64::<LittleEndian>()?;

        let mut memo_bytes = [0u8; 512];
        reader.read_exact(&mut memo_bytes)?;
        let memo = match MemoBytes::from_bytes(&memo_bytes) {
            Ok(mb) => match Memo::try_from(mb.clone()) {
                Ok(m) => Ok(m),
                Err(_) => Ok(Memo::Future(mb)),
            },
            Err(e) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Couldn't create memo: {}", e),
            )),
        }?;

        Ok(OutgoingTxMetadata {
            to_address: address,
            value,
            memo,
            ua: None,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        // Strings are written as len + utf8
        match &self.ua {
            None => {
                writer.write_u64::<LittleEndian>(self.to_address.as_bytes().len() as u64)?;
                writer.write_all(self.to_address.as_bytes())?;
            }
            Some(ua) => {
                writer.write_u64::<LittleEndian>(ua.as_bytes().len() as u64)?;
                writer.write_all(ua.as_bytes())?;
            }
        }
        writer.write_u64::<LittleEndian>(self.value)?;
        writer.write_all(self.memo.encode().as_array())
    }
}

pub struct TransactionMetadata {
    // Block in which this tx was included
    pub block_height: BlockHeight,

    // Is this Tx unconfirmed (i.e., not yet mined)
    pub unconfirmed: bool,

    // Timestamp of Tx. Added in v4
    pub datetime: u64,

    // Txid of this transaction. It's duplicated here (It is also the Key in the HashMap that points to this
    // WalletTx in LightWallet::txs)
    pub txid: TxId,

    // List of all nullifiers spent in this Tx. These nullifiers belong to the wallet.
    pub spent_sapling_nullifiers: Vec<SaplingNullifier>,

    // List of all nullifiers spent in this Tx. These nullifiers belong to the wallet.
    pub spent_orchard_nullifiers: Vec<OrchardNullifier>,

    // List of all sapling notes received in this tx. Some of these might be change notes.
    pub sapling_notes: Vec<ReceivedSaplingNoteAndMetadata>,

    // List of all sapling notes received in this tx. Some of these might be change notes.
    pub orchard_notes: Vec<ReceivedOrchardNoteAndMetadata>,

    // List of all Utxos received in this Tx. Some of these might be change notes
    pub utxos: Vec<Utxo>,

    // Total value of all the sapling nullifiers that were spent in this Tx
    pub total_sapling_value_spent: u64,

    // Total value of all the orchard nullifiers that were spent in this Tx
    pub total_orchard_value_spent: u64,

    // Total amount of transparent funds that belong to us that were spent in this Tx.
    pub total_transparent_value_spent: u64,

    // All outgoing sends to addresses outside this wallet
    pub outgoing_metadata: Vec<OutgoingTxMetadata>,

    // Whether this TxID was downloaded from the server and scanned for Memos
    pub full_tx_scanned: bool,

    // Price of Zec when this Tx was created
    pub zec_price: Option<f64>,
}

impl TransactionMetadata {
    pub fn serialized_version() -> u64 {
        return 23;
    }

    pub fn new_txid(txid: &Vec<u8>) -> TxId {
        let mut txid_bytes = [0u8; 32];
        txid_bytes.copy_from_slice(txid);
        TxId::from_bytes(txid_bytes)
    }

    pub fn total_value_spent(&self) -> u64 {
        self.value_spent_by_pool().iter().sum()
    }
    pub fn value_spent_by_pool(&self) -> [u64; 3] {
        [
            self.total_transparent_value_spent,
            self.total_sapling_value_spent,
            self.total_orchard_value_spent,
        ]
    }

    pub fn get_price(datetime: u64, price: &WalletZecPriceInfo) -> Option<f64> {
        match price.zec_price {
            None => None,
            Some((t, p)) => {
                // If the price was fetched within 24 hours of this Tx, we use the "current" price
                // else, we mark it as None, for the historical price fetcher to get
                // TODO:  Investigate the state of "the historical price fetcher".
                if (t as i64 - datetime as i64).abs() < 24 * 60 * 60 {
                    Some(p)
                } else {
                    None
                }
            }
        }
    }

    pub fn new(
        height: BlockHeight,
        datetime: u64,
        transaction_id: &TxId,
        unconfirmed: bool,
    ) -> Self {
        TransactionMetadata {
            block_height: height,
            unconfirmed,
            datetime,
            txid: transaction_id.clone(),
            spent_sapling_nullifiers: vec![],
            spent_orchard_nullifiers: vec![],
            sapling_notes: vec![],
            orchard_notes: vec![],
            utxos: vec![],
            total_transparent_value_spent: 0,
            total_sapling_value_spent: 0,
            total_orchard_value_spent: 0,
            outgoing_metadata: vec![],
            full_tx_scanned: false,
            zec_price: None,
        }
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
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

        let sapling_notes =
            Vector::read(&mut reader, |r| ReceivedSaplingNoteAndMetadata::read(r, ()))?;
        let orchard_notes = if version > 22 {
            Vector::read(&mut reader, |r| ReceivedOrchardNoteAndMetadata::read(r, ()))?
        } else {
            vec![]
        };
        let utxos = Vector::read(&mut reader, |r| Utxo::read(r))?;

        let total_sapling_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_transparent_value_spent = reader.read_u64::<LittleEndian>()?;
        let total_orchard_value_spent = if version >= 22 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };

        // Outgoing metadata was only added in version 2
        let outgoing_metadata = Vector::read(&mut reader, |r| OutgoingTxMetadata::read(r))?;

        let full_tx_scanned = reader.read_u8()? > 0;

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
                Ok(SaplingNullifier(n))
            })?
        };

        let spent_orchard_nullifiers = if version <= 21 {
            vec![]
        } else {
            Vector::read(&mut reader, |r| {
                let mut n = [0u8; 32];
                r.read_exact(&mut n)?;
                Ok(OrchardNullifier::from_bytes(&n).unwrap())
            })?
        };

        Ok(Self {
            block_height: block,
            unconfirmed,
            datetime,
            txid: transaction_id,
            sapling_notes,
            orchard_notes,
            utxos,
            spent_sapling_nullifiers,
            spent_orchard_nullifiers,
            total_sapling_value_spent,
            total_transparent_value_spent,
            total_orchard_value_spent,
            outgoing_metadata,
            full_tx_scanned,
            zec_price,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        let block: u32 = self.block_height.into();
        writer.write_i32::<LittleEndian>(block as i32)?;

        writer.write_u8(if self.unconfirmed { 1 } else { 0 })?;

        writer.write_u64::<LittleEndian>(self.datetime)?;

        writer.write_all(self.txid.as_ref())?;

        Vector::write(&mut writer, &self.sapling_notes, |w, nd| nd.write(w))?;
        Vector::write(&mut writer, &self.orchard_notes, |w, nd| nd.write(w))?;
        Vector::write(&mut writer, &self.utxos, |w, u| u.write(w))?;

        for pool in self.value_spent_by_pool() {
            writer.write_u64::<LittleEndian>(pool)?;
        }

        // Write the outgoing metadata
        Vector::write(&mut writer, &self.outgoing_metadata, |w, om| om.write(w))?;

        writer.write_u8(if self.full_tx_scanned { 1 } else { 0 })?;

        Optional::write(&mut writer, self.zec_price, |w, p| {
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

    pub(super) fn add_spent_nullifier(&mut self, nullifier: PoolNullifier, value: u64) {
        match nullifier {
            PoolNullifier::Sapling(nf) => {
                self.spent_sapling_nullifiers.push(nf);
                self.total_sapling_value_spent += value;
            }
            PoolNullifier::Orchard(nf) => {
                self.spent_orchard_nullifiers.push(nf);
                self.total_orchard_value_spent += value;
            }
        }
    }
}

pub struct SpendableSaplingNote {
    pub transaction_id: TxId,
    pub nullifier: SaplingNullifier,
    pub diversifier: SaplingDiversifier,
    pub note: SaplingNote,
    pub witness: IncrementalWitness<SaplingNode>,
    pub extsk: ExtendedSpendingKey,
}

pub struct SpendableOrchardNote {
    pub transaction_id: TxId,
    pub nullifier: OrchardNullifier,
    pub diversifier: OrchardDiversifier,
    pub note: OrchardNote,
    pub witness: IncrementalWitness<MerkleHashOrchard>,
    pub spend_key: OrchardSpendingKey,
}

// Struct that tracks the latest and historical price of ZEC in the wallet
#[derive(Clone, Debug)]
pub struct WalletZecPriceInfo {
    // Latest price of ZEC and when it was fetched
    pub zec_price: Option<(u64, f64)>,

    // Wallet's currency. All the prices are in this currency
    pub currency: String,

    // When the last time historical prices were fetched
    pub last_historical_prices_fetched_at: Option<u64>,

    // Historical prices retry count
    pub historical_prices_retry_count: u64,
}

impl WalletZecPriceInfo {
    pub fn new() -> Self {
        Self {
            zec_price: None,
            currency: "USD".to_string(), // Only USD is supported right now.
            last_historical_prices_fetched_at: None,
            historical_prices_retry_count: 0,
        }
    }

    pub fn serialized_version() -> u64 {
        return 20;
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        if version > Self::serialized_version() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Can't read ZecPriceInfo because of incorrect version",
            ));
        }

        // The "current" zec price is not persisted, since it is almost certainly outdated
        let zec_price = None;

        // Currency is only USD for now
        let currency = "USD".to_string();

        let last_historical_prices_fetched_at =
            Optional::read(&mut reader, |r| r.read_u64::<LittleEndian>())?;
        let historical_prices_retry_count = reader.read_u64::<LittleEndian>()?;

        Ok(Self {
            zec_price,
            currency,
            last_historical_prices_fetched_at,
            historical_prices_retry_count,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // We don't write the currency zec price or the currency yet.
        Optional::write(
            &mut writer,
            self.last_historical_prices_fetched_at,
            |w, t| w.write_u64::<LittleEndian>(t),
        )?;
        writer.write_u64::<LittleEndian>(self.historical_prices_retry_count)?;

        Ok(())
    }
}

#[test]
fn read_write_empty_sapling_tree() {
    let mut buffer = Vec::new();

    CommitmentTree::<SaplingNode>::empty()
        .write(&mut buffer)
        .unwrap();
    assert_eq!(
        CommitmentTree::<SaplingNode>::empty(),
        CommitmentTree::<SaplingNode>::read(&mut buffer.as_slice()).unwrap()
    )
}

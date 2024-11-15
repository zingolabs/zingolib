//! TODO: Add Mod Description Here!
use super::traits::ToBytes;

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::frontier::CommitmentTree;
use incrementalmerkletree::witness::IncrementalWitness;
use incrementalmerkletree::{Hashable, Position};

use json::JsonValue;
use prost::Message;

use std::convert::TryFrom;
use std::io::{self, Read, Write};
use zcash_client_backend::{
    proto::compact_formats::CompactBlock, wallet::TransparentAddressMetadata,
};

use zcash_encoding::{CompactSize, Optional, Vector};

use zcash_primitives::merkle_tree::{read_commitment_tree, write_commitment_tree};
use zcash_primitives::{legacy::TransparentAddress, memo::MemoBytes};
use zcash_primitives::{memo::Memo, transaction::TxId};

pub use crate::wallet::transaction_record::TransactionRecord; // TODO: is this necessary? can we import this directly where its used?

/// TODO: Add Doc Comment Here!
pub const COMMITMENT_TREE_LEVELS: u8 = 32;
/// TODO: Add Doc Comment Here!
pub const MAX_SHARD_LEVEL: u8 = 16;

/// This type is motivated by the IPC architecture where (currently) channels traffic in
/// `(TxId, WalletNullifier, BlockHeight, Option<u32>)`.  This enum permits a single channel
/// type to handle nullifiers from different domains.
/// <https://github.com/zingolabs/zingolib/issues/64>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolNullifier {
    /// TODO: Add Doc Comment Here!
    Sapling(sapling_crypto::Nullifier),
    /// TODO: Add Doc Comment Here!
    Orchard(orchard::note::Nullifier),
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

/// TODO: Add Doc Comment Here!
#[derive(Clone, Debug, PartialEq)]
pub struct BlockData {
    /// TODO: Add Doc Comment Here!
    pub(crate) ecb: Vec<u8>,
    /// TODO: Add Doc Comment Here!
    pub height: u64,
}

impl BlockData {
    /// TODO: Add Doc Comment Here!
    pub fn serialized_version() -> u64 {
        20
    }

    pub(crate) fn new_with(height: u64, hash: &[u8]) -> Self {
        let hash = hash.iter().copied().rev().collect::<Vec<_>>();

        let cb = CompactBlock {
            hash,
            ..Default::default()
        };

        let mut ecb = vec![];
        cb.encode(&mut ecb).unwrap();

        Self { ecb, height }
    }

    pub(crate) fn new(mut cb: CompactBlock) -> Self {
        for compact_transaction in &mut cb.vtx {
            for co in &mut compact_transaction.outputs {
                co.ciphertext.clear();
                co.ephemeral_key.clear();
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

    /// TODO: Add Doc Comment Here!
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let height = reader.read_i32::<LittleEndian>()? as u64;

        let mut hash_bytes = [0; 32];
        reader.read_exact(&mut hash_bytes)?;
        hash_bytes.reverse();

        // We don't need this, but because of a quirk, the version is stored later, so we can't actually
        // detect the version here. So we write an empty tree and read it back here
        let tree: sapling_crypto::CommitmentTree = read_commitment_tree(&mut reader)?;
        let _tree = if tree.size() == 0 { None } else { Some(tree) };

        let version = reader.read_u64::<LittleEndian>()?;

        let ecb = if version <= 11 {
            vec![]
        } else {
            Vector::read(&mut reader, |r| r.read_u8())?
        };

        if ecb.is_empty() {
            Ok(BlockData::new_with(height, &hash_bytes))
        } else {
            Ok(BlockData { ecb, height })
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_i32::<LittleEndian>(self.height as i32)?;

        let hash_bytes: Vec<_> = hex::decode(self.hash())
            .unwrap()
            .into_iter()
            .rev()
            .collect();
        writer.write_all(&hash_bytes[..])?;

        write_commitment_tree(
            &CommitmentTree::<sapling_crypto::Node, 32>::empty(),
            &mut writer,
        )?;
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // Write the ecb as well
        Vector::write(&mut writer, &self.ecb, |w, b| w.write_u8(*b))?;

        Ok(())
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Clone)]
pub struct WitnessCache<Node: Hashable> {
    /// TODO: Add Doc Comment Here!
    pub(crate) witnesses: Vec<IncrementalWitness<Node, 32>>,
    /// TODO: Add Doc Comment Here!
    pub top_height: u64,
}

impl<Node: Hashable> std::fmt::Debug for WitnessCache<Node> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WitnessCache")
            .field("witnesses", &self.witnesses.len())
            .field("top_height", &self.top_height)
            .finish_non_exhaustive()
    }
}

impl<Node: Hashable> WitnessCache<Node> {
    /// TODO: Add Doc Comment Here!
    pub fn new(witnesses: Vec<IncrementalWitness<Node, 32>>, top_height: u64) -> Self {
        Self {
            witnesses,
            top_height,
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn empty() -> Self {
        Self {
            witnesses: vec![],
            top_height: 0,
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn len(&self) -> usize {
        self.witnesses.len()
    }

    /// TODO: Add Doc Comment Here!
    pub fn is_empty(&self) -> bool {
        self.witnesses.is_empty()
    }

    /// TODO: Add Doc Comment Here!
    pub fn clear(&mut self) {
        self.witnesses.clear();
    }

    /// TODO: Add Doc Comment Here!
    pub fn get(&self, i: usize) -> Option<&IncrementalWitness<Node, 32>> {
        self.witnesses.get(i)
    }

    /// TODO: Add Doc Comment Here!
    #[cfg(test)]
    pub fn get_from_last(&self, i: usize) -> Option<&IncrementalWitness<Node, 32>> {
        self.witnesses.get(self.len() - i - 1)
    }

    /// TODO: Add Doc Comment Here!
    pub fn last(&self) -> Option<&IncrementalWitness<Node, 32>> {
        self.witnesses.last()
    }

    /// TODO: Add Doc Comment Here!
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

// Reading a note also needs the corresponding address to read from.
pub(crate) fn read_sapling_rseed<R: Read>(mut reader: R) -> io::Result<sapling_crypto::Rseed> {
    let note_type = reader.read_u8()?;

    let mut r_bytes: [u8; 32] = [0; 32];
    reader.read_exact(&mut r_bytes)?;

    let r = match note_type {
        1 => sapling_crypto::Rseed::BeforeZip212(jubjub::Fr::from_bytes(&r_bytes).unwrap()),
        2 => sapling_crypto::Rseed::AfterZip212(r_bytes),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "Bad note type")),
    };

    Ok(r)
}

pub(crate) fn write_sapling_rseed<W: Write>(
    mut writer: W,
    rseed: &sapling_crypto::Rseed,
) -> io::Result<()> {
    let note_type = match rseed {
        sapling_crypto::Rseed::BeforeZip212(_) => 1,
        sapling_crypto::Rseed::AfterZip212(_) => 2,
    };
    writer.write_u8(note_type)?;

    match rseed {
        sapling_crypto::Rseed::BeforeZip212(fr) => writer.write_all(&fr.to_bytes()),
        sapling_crypto::Rseed::AfterZip212(b) => writer.write_all(b),
    }
}

/// Only for TransactionRecords *from* "this" capability
#[derive(Clone, Debug)]
pub struct OutgoingTxData {
    /// TODO: Add Doc Comment Here!
    pub recipient_address: String,
    /// Amount to this receiver
    pub value: u64,
    /// Note to the receiver, why not an option?
    pub memo: Memo,
    /// What if it wasn't provided?  How does this relate to
    /// recipient_address?
    pub recipient_ua: Option<String>,
    /// This output's index in its containing transaction
    pub output_index: Option<u64>,
}
impl std::fmt::Display for OutgoingTxData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Format the recipient address or unified address if provided.
        let address_display = if let Some(ref ua) = self.recipient_ua {
            format!("unified address: {}", ua)
        } else {
            format!("recipient address: {}", self.recipient_address)
        };
        let memo_text = if let Memo::Text(mt) = self.memo.clone() {
            mt.to_string()
        } else {
            "not a text memo".to_string()
        };

        write!(
            f,
            "\t{{
            {}
            value: {}
            memo: {}
        }}",
            address_display, self.value, memo_text
        )
    }
}
impl From<OutgoingTxData> for JsonValue {
    fn from(outgoing_tx_data: OutgoingTxData) -> Self {
        let address = outgoing_tx_data
            .recipient_ua
            .unwrap_or(outgoing_tx_data.recipient_address);
        let memo = if let Memo::Text(memo_text) = outgoing_tx_data.memo {
            Some(memo_text.to_string())
        } else {
            None
        };
        json::object! {
            "address" => address,
            "value" => outgoing_tx_data.value,
            "memo" => memo,
        }
    }
}
impl PartialEq for OutgoingTxData {
    fn eq(&self, other: &Self) -> bool {
        (self.recipient_address == other.recipient_address
            || self.recipient_ua == other.recipient_ua)
            && self.value == other.value
            && self.memo == other.memo
    }
}
impl OutgoingTxData {
    const VERSION: u64 = 0;
    /// Before version 0, OutgoingTxData didn't have a version field
    pub fn read_old<R: Read>(mut reader: R) -> io::Result<Self> {
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

        Ok(OutgoingTxData {
            recipient_address: address,
            value,
            memo,
            recipient_ua: None,
            output_index: None,
        })
    }

    /// Read an OutgoingTxData from its serialized
    /// representation
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let _external_version = CompactSize::read(&mut reader)?;
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
        let output_index = Optional::read(&mut reader, CompactSize::read)?;

        Ok(OutgoingTxData {
            recipient_address: address,
            value,
            memo,
            recipient_ua: None,
            output_index,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        CompactSize::write(&mut writer, Self::VERSION as usize)?;
        // Strings are written as len + utf8
        match &self.recipient_ua {
            None => {
                writer.write_u64::<LittleEndian>(self.recipient_address.as_bytes().len() as u64)?;
                writer.write_all(self.recipient_address.as_bytes())?;
            }
            Some(ua) => {
                writer.write_u64::<LittleEndian>(ua.as_bytes().len() as u64)?;
                writer.write_all(ua.as_bytes())?;
            }
        }
        writer.write_u64::<LittleEndian>(self.value)?;
        writer.write_all(self.memo.encode().as_array())?;
        Optional::write(&mut writer, self.output_index, |w, output_index| {
            CompactSize::write(w, output_index as usize)
        })
    }
}

/// Wraps a vec of outgoing transaction datas for the implementation of std::fmt::Display
pub struct OutgoingTxDataSummaries(Vec<OutgoingTxData>);

impl std::fmt::Display for OutgoingTxDataSummaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for outgoing_tx_data in &self.0 {
            write!(f, "\n{}", outgoing_tx_data)?;
        }
        Ok(())
    }
}

/// TODO: Add Mod Description Here!
pub mod finsight {
    /// TODO: Add Doc Comment Here!
    pub struct ValuesSentToAddress(pub std::collections::HashMap<String, Vec<u64>>);
    /// TODO: Add Doc Comment Here!
    pub struct TotalValueToAddress(pub std::collections::HashMap<String, u64>);
    /// TODO: Add Doc Comment Here!
    pub struct TotalSendsToAddress(pub std::collections::HashMap<String, u64>);
    /// TODO: Add Doc Comment Here!
    #[derive(Debug)]
    pub struct TotalMemoBytesToAddress(pub std::collections::HashMap<String, usize>);

    impl From<TotalMemoBytesToAddress> for json::JsonValue {
        fn from(value: TotalMemoBytesToAddress) -> Self {
            let mut jsonified = json::object!();
            let hm = value.0;
            for (key, val) in hm.iter() {
                jsonified[key] = json::JsonValue::from(*val);
            }
            jsonified
        }
    }

    impl From<TotalValueToAddress> for json::JsonValue {
        fn from(value: TotalValueToAddress) -> Self {
            let mut jsonified = json::object!();
            let hm = value.0;
            for (key, val) in hm.iter() {
                jsonified[key] = json::JsonValue::from(*val);
            }
            jsonified
        }
    }

    impl From<TotalSendsToAddress> for json::JsonValue {
        fn from(value: TotalSendsToAddress) -> Self {
            let mut jsonified = json::object!();
            let hm = value.0;
            for (key, val) in hm.iter() {
                jsonified[key] = json::JsonValue::from(*val);
            }
            jsonified
        }
    }
}

/// A mod designed for conveniently displaying information to the user or converting to JSON to pass through an FFI.
/// A "snapshot" of the state of the items in the wallet at the time the summary was constructed.
/// Not to be used for internal logic in the system.
pub mod summaries {
    use crate::config::ChainType;
    use chrono::DateTime;
    use json::JsonValue;
    use zcash_primitives::{consensus::BlockHeight, memo::Memo, transaction::TxId};
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::{
        error::BuildError,
        utils::build_method,
        wallet::{
            data::OutgoingTxDataSummaries,
            notes::OutputInterface as _,
            transaction_record::{SendType, TransactionKind},
            transaction_records_by_id::TransactionRecordsById,
        },
    };

    use super::{OutgoingTxData, TransactionRecord};

    /// A value transfer is a note group abstraction.
    /// A group of all notes sent to a specific address in a transaction.
    #[derive(PartialEq)]
    pub struct ValueTransfer {
        txid: TxId,
        datetime: u64,
        status: ConfirmationStatus,
        blockheight: BlockHeight,
        transaction_fee: Option<u64>,
        zec_price: Option<f64>,
        kind: ValueTransferKind,
        value: u64,
        recipient_address: Option<String>,
        pool_received: Option<String>,
        memos: Vec<String>,
    }

    impl ValueTransfer {
        /// Gets txid
        pub fn txid(&self) -> TxId {
            self.txid
        }
        /// Gets datetime
        pub fn datetime(&self) -> u64 {
            self.datetime
        }
        /// Gets confirmation status
        pub fn status(&self) -> ConfirmationStatus {
            self.status
        }
        /// Gets blockheight
        pub fn blockheight(&self) -> BlockHeight {
            self.blockheight
        }
        /// Gets transaction fee
        pub fn transaction_fee(&self) -> Option<u64> {
            self.transaction_fee
        }
        /// Gets zec price in USD
        pub fn zec_price(&self) -> Option<f64> {
            self.zec_price
        }
        /// Gets value transfer kind
        pub fn kind(&self) -> ValueTransferKind {
            self.kind
        }
        /// Gets value
        pub fn value(&self) -> u64 {
            self.value
        }
        /// Gets recipient address
        pub fn recipient_address(&self) -> Option<&str> {
            self.recipient_address.as_deref()
        }
        /// Gets pool received
        pub fn pool_received(&self) -> Option<&str> {
            self.pool_received.as_deref()
        }
        /// Gets memos
        pub fn memos(&self) -> Vec<&str> {
            self.memos.iter().map(|s| s.as_str()).collect()
        }
    }

    impl std::fmt::Debug for ValueTransfer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ValueTransfer")
                .field("txid", &self.txid)
                .field("datetime", &self.datetime)
                .field("status", &self.status)
                .field("blockheight", &self.blockheight)
                .field("transaction_fee", &self.transaction_fee)
                .field("zec_price", &self.zec_price)
                .field("kind", &self.kind)
                .field("value", &self.value)
                .field("recipient_address", &self.recipient_address)
                .field("pool_received", &self.pool_received)
                .field("memos", &self.memos)
                .finish()
        }
    }

    impl std::fmt::Display for ValueTransfer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let datetime = if let Some(dt) = DateTime::from_timestamp(self.datetime as i64, 0) {
                format!("{}", dt)
            } else {
                "not available".to_string()
            };
            let transaction_fee = if let Some(f) = self.transaction_fee() {
                f.to_string()
            } else {
                "not available".to_string()
            };
            let zec_price = if let Some(price) = self.zec_price() {
                price.to_string()
            } else {
                "not available".to_string()
            };
            let recipient_address = if let Some(addr) = self.recipient_address() {
                addr.to_string()
            } else {
                "not available".to_string()
            };
            let pool_received = if let Some(pool) = self.pool_received() {
                pool.to_string()
            } else {
                "not available".to_string()
            };
            let mut memos = String::new();
            for (index, memo) in self.memos().into_iter().enumerate() {
                memos.push_str(&format!("\n\tmemo {}: {}", (index + 1), memo));
            }
            write!(
                f,
                "{{
    txid: {}
    datetime: {}
    status: {}
    blockheight: {}
    transaction fee: {}
    zec price: {}
    kind: {}
    value: {}
    recipient_address: {}
    pool_received: {}
    memos: {}
}}",
                self.txid,
                datetime,
                self.status,
                u64::from(self.blockheight),
                transaction_fee,
                zec_price,
                self.kind,
                self.value,
                recipient_address,
                pool_received,
                memos
            )
        }
    }

    impl From<ValueTransfer> for JsonValue {
        fn from(value_transfer: ValueTransfer) -> Self {
            json::object! {
                "txid" => value_transfer.txid.to_string(),
                "datetime" => value_transfer.datetime,
                "status" => value_transfer.status.to_string(),
                "blockheight" => u64::from(value_transfer.blockheight),
                "transaction_fee" => value_transfer.transaction_fee,
                "zec_price" => value_transfer.zec_price,
                "kind" => value_transfer.kind.to_string(),
                "value" => value_transfer.value,
                "recipient_address" => value_transfer.recipient_address,
                "pool_received" => value_transfer.pool_received,
                "memos" => value_transfer.memos
            }
        }
    }

    /// A wrapper struct for implementing display and json on a vec of value trasnfers
    #[derive(PartialEq, Debug)]
    pub struct ValueTransfers(pub Vec<ValueTransfer>);

    impl ValueTransfers {
        /// Creates a new ValueTransfer
        pub fn new(value_transfers: Vec<ValueTransfer>) -> Self {
            ValueTransfers(value_transfers)
        }
        /// Implicitly dispatch to the wrapped data
        pub fn iter(&self) -> std::slice::Iter<ValueTransfer> {
            self.0.iter()
        }
    }

    impl std::fmt::Display for ValueTransfers {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for value_transfer in &self.0 {
                write!(f, "\n{}", value_transfer)?;
            }
            Ok(())
        }
    }

    impl From<ValueTransfers> for JsonValue {
        fn from(value_transfers: ValueTransfers) -> Self {
            let value_transfers: Vec<JsonValue> =
                value_transfers.0.into_iter().map(JsonValue::from).collect();
            json::object! {
                "value_transfers" => value_transfers
            }
        }
    }

    /// Builds ValueTransfer from builder
    pub struct ValueTransferBuilder {
        txid: Option<TxId>,
        datetime: Option<u64>,
        status: Option<ConfirmationStatus>,
        blockheight: Option<BlockHeight>,
        transaction_fee: Option<Option<u64>>,
        zec_price: Option<Option<f64>>,
        kind: Option<ValueTransferKind>,
        value: Option<u64>,
        recipient_address: Option<Option<String>>,
        pool_received: Option<Option<String>>,
        memos: Option<Vec<String>>,
    }

    impl ValueTransferBuilder {
        /// Creates a new ValueTransfer builder
        pub fn new() -> ValueTransferBuilder {
            ValueTransferBuilder {
                txid: None,
                datetime: None,
                status: None,
                blockheight: None,
                transaction_fee: None,
                zec_price: None,
                kind: None,
                value: None,
                recipient_address: None,
                pool_received: None,
                memos: None,
            }
        }

        build_method!(txid, TxId);
        build_method!(datetime, u64);
        build_method!(status, ConfirmationStatus);
        build_method!(blockheight, BlockHeight);
        build_method!(transaction_fee, Option<u64>);
        build_method!(zec_price, Option<f64>);
        build_method!(kind, ValueTransferKind);
        build_method!(value, u64);
        build_method!(recipient_address, Option<String>);
        build_method!(pool_received, Option<String>);
        build_method!(memos, Vec<String>);

        /// Builds a ValueTransfer from builder
        pub fn build(&self) -> Result<ValueTransfer, BuildError> {
            Ok(ValueTransfer {
                txid: self
                    .txid
                    .ok_or(BuildError::MissingField("txid".to_string()))?,
                datetime: self
                    .datetime
                    .ok_or(BuildError::MissingField("datetime".to_string()))?,
                status: self
                    .status
                    .ok_or(BuildError::MissingField("status".to_string()))?,
                blockheight: self
                    .blockheight
                    .ok_or(BuildError::MissingField("blockheight".to_string()))?,
                transaction_fee: self
                    .transaction_fee
                    .ok_or(BuildError::MissingField("transaction_fee".to_string()))?,
                zec_price: self
                    .zec_price
                    .ok_or(BuildError::MissingField("zec_price".to_string()))?,
                kind: self
                    .kind
                    .ok_or(BuildError::MissingField("kind".to_string()))?,
                value: self
                    .value
                    .ok_or(BuildError::MissingField("value".to_string()))?,
                recipient_address: self
                    .recipient_address
                    .clone()
                    .ok_or(BuildError::MissingField("recipient_address".to_string()))?,
                pool_received: self
                    .pool_received
                    .clone()
                    .ok_or(BuildError::MissingField("pool_received".to_string()))?,
                memos: self
                    .memos
                    .clone()
                    .ok_or(BuildError::MissingField("memos".to_string()))?,
            })
        }
    }

    impl Default for ValueTransferBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Variants of within transaction outputs grouped by receiver
    /// non_exhaustive to permit expanding to include an
    /// Deshield variant fo sending to transparent
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum ValueTransferKind {
        /// The recipient is different than this creator
        Sent(SentValueTransfer),
        /// The wallet capability is receiving funds in a transaction
        /// that was created by a different capability
        Received,
    }
    /// There are 2 kinds of sent value to-other and to-self
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum SentValueTransfer {
        /// Transaction is sending funds to recipient other than the creator
        Send,
        /// The recipient is the creator and the transaction has no recipients that are not the creator
        SendToSelf(SelfSendValueTransfer),
    }
    /// There are 4 kinds of self sends (so far)
    #[non_exhaustive]
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum SelfSendValueTransfer {
        /// Explicit memo-less value sent to self
        Basic,
        /// The recipient is the creator and this is a shield transaction
        Shield,
        /// The recipient is the creator and is receiving at least 1 note with a TEXT memo
        MemoToSelf,
        /// The recipient is an "ephemeral" 320 address
        Rejection,
    }

    impl std::fmt::Display for ValueTransferKind {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            match self {
                ValueTransferKind::Received => write!(f, "received"),
                ValueTransferKind::Sent(sent) => match sent {
                    SentValueTransfer::Send => write!(f, "sent"),
                    SentValueTransfer::SendToSelf(selfsend) => match selfsend {
                        SelfSendValueTransfer::Basic => write!(f, "basic"),
                        SelfSendValueTransfer::Shield => write!(f, "shield"),
                        SelfSendValueTransfer::MemoToSelf => write!(f, "memo-to-self"),
                        SelfSendValueTransfer::Rejection => write!(f, "rejection"),
                    },
                },
            }
        }
    }

    /// Basic transaction summary interface
    pub trait TransactionSummaryInterface {
        /// Gets txid
        fn txid(&self) -> TxId;
        /// Gets datetime
        fn datetime(&self) -> u64;
        /// Gets confirmation status
        fn status(&self) -> ConfirmationStatus;
        /// Gets blockheight
        fn blockheight(&self) -> BlockHeight;
        /// Gets transaction kind
        fn kind(&self) -> TransactionKind;
        /// Gets value
        fn value(&self) -> u64;
        /// Gets fee
        fn fee(&self) -> Option<u64>;
        /// Gets zec price in USD
        fn zec_price(&self) -> Option<f64>;
        /// Gets slice of orchard note summaries
        fn orchard_notes(&self) -> &[OrchardNoteSummary];
        /// Gets slice of sapling note summaries
        fn sapling_notes(&self) -> &[SaplingNoteSummary];
        /// Gets slice of transparent coin summaries
        fn transparent_coins(&self) -> &[TransparentCoinSummary];
        /// Gets slice of outgoing transaction data
        fn outgoing_tx_data(&self) -> &[OutgoingTxData];
        /// Depending on the relationship of this capability to the
        /// receiver capability, assign polarity to value transferred.
        /// Returns None if fields expecting Some(_) are None
        fn balance_delta(&self) -> Option<i64> {
            match self.kind() {
                TransactionKind::Sent(SendType::Send) => {
                    self.fee().map(|fee| -((self.value() + fee) as i64))
                }
                TransactionKind::Sent(SendType::Shield)
                | TransactionKind::Sent(SendType::SendToSelf) => {
                    self.fee().map(|fee| -(fee as i64))
                }
                TransactionKind::Received => Some(self.value() as i64),
            }
        }
        /// Prepares the fields in the summary for display
        fn prepare_for_display(
            &self,
        ) -> (
            String,
            String,
            String,
            OrchardNoteSummaries,
            SaplingNoteSummaries,
            TransparentCoinSummaries,
            OutgoingTxDataSummaries,
        ) {
            let datetime = if let Some(dt) = DateTime::from_timestamp(self.datetime() as i64, 0) {
                format!("{}", dt)
            } else {
                "not available".to_string()
            };
            let fee = if let Some(f) = self.fee() {
                f.to_string()
            } else {
                "not available".to_string()
            };
            let zec_price = if let Some(price) = self.zec_price() {
                price.to_string()
            } else {
                "not available".to_string()
            };
            let orchard_notes = OrchardNoteSummaries(self.orchard_notes().to_vec());
            let sapling_notes = SaplingNoteSummaries(self.sapling_notes().to_vec());
            let transparent_coins = TransparentCoinSummaries(self.transparent_coins().to_vec());
            let outgoing_tx_data_summaries =
                OutgoingTxDataSummaries(self.outgoing_tx_data().to_vec());

            (
                datetime,
                fee,
                zec_price,
                orchard_notes,
                sapling_notes,
                transparent_coins,
                outgoing_tx_data_summaries,
            )
        }
    }

    /// Transaction summary.
    /// A struct designed for conveniently displaying information to the user or converting to JSON to pass through an FFI.
    /// A "snapshot" of the state of a transaction in the wallet at the time the summary was constructed.
    /// Not to be used for internal logic in the system.
    #[derive(Clone, PartialEq, Debug)]
    pub struct TransactionSummary {
        txid: TxId,
        datetime: u64,
        status: ConfirmationStatus,
        blockheight: BlockHeight,
        kind: TransactionKind,
        value: u64,
        fee: Option<u64>,
        zec_price: Option<f64>,
        orchard_notes: Vec<OrchardNoteSummary>,
        sapling_notes: Vec<SaplingNoteSummary>,
        transparent_coins: Vec<TransparentCoinSummary>,
        outgoing_tx_data: Vec<OutgoingTxData>,
    }

    impl TransactionSummaryInterface for TransactionSummary {
        fn txid(&self) -> TxId {
            self.txid
        }
        fn datetime(&self) -> u64 {
            self.datetime
        }
        fn status(&self) -> ConfirmationStatus {
            self.status
        }
        fn blockheight(&self) -> BlockHeight {
            self.blockheight
        }
        fn kind(&self) -> TransactionKind {
            self.kind
        }
        fn value(&self) -> u64 {
            self.value
        }
        fn fee(&self) -> Option<u64> {
            self.fee
        }
        fn zec_price(&self) -> Option<f64> {
            self.zec_price
        }
        fn orchard_notes(&self) -> &[OrchardNoteSummary] {
            &self.orchard_notes
        }
        fn sapling_notes(&self) -> &[SaplingNoteSummary] {
            &self.sapling_notes
        }
        fn transparent_coins(&self) -> &[TransparentCoinSummary] {
            &self.transparent_coins
        }
        fn outgoing_tx_data(&self) -> &[OutgoingTxData] {
            &self.outgoing_tx_data
        }
    }

    impl std::fmt::Display for TransactionSummary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let (
                datetime,
                fee,
                zec_price,
                orchard_notes,
                sapling_notes,
                transparent_coins,
                outgoing_tx_data_summaries,
            ) = self.prepare_for_display();
            write!(
                f,
                "{{
    txid: {}
    datetime: {}
    status: {}
    blockheight: {}
    kind: {}
    value: {}
    fee: {}
    zec price: {}
    orchard notes: {}
    sapling notes: {}
    transparent coins: {}
    outgoing data: {}
}}",
                self.txid,
                datetime,
                self.status,
                u64::from(self.blockheight),
                self.kind,
                self.value,
                fee,
                zec_price,
                orchard_notes,
                sapling_notes,
                transparent_coins,
                outgoing_tx_data_summaries
            )
        }
    }

    impl From<TransactionSummary> for JsonValue {
        fn from(transaction: TransactionSummary) -> Self {
            json::object! {
                "txid" => transaction.txid.to_string(),
                "datetime" => transaction.datetime,
                "status" => transaction.status.to_string(),
                "blockheight" => u64::from(transaction.blockheight),
                "kind" => transaction.kind.to_string(),
                "value" => transaction.value,
                "fee" => transaction.fee,
                "zec_price" => transaction.zec_price,
                "orchard_notes" => JsonValue::from(transaction.orchard_notes),
                "sapling_notes" => JsonValue::from(transaction.sapling_notes),
                "transparent_coins" => JsonValue::from(transaction.transparent_coins),
                "outgoing_tx_data" => JsonValue::from(transaction.outgoing_tx_data),
            }
        }
    }

    /// Wraps a vec of transaction summaries for the implementation of std::fmt::Display
    #[derive(PartialEq, Debug)]
    pub struct TransactionSummaries(pub Vec<TransactionSummary>);

    impl TransactionSummaries {
        /// Creates a new TransactionSummaries struct
        pub fn new(transaction_summaries: Vec<TransactionSummary>) -> Self {
            TransactionSummaries(transaction_summaries)
        }
        /// Implicitly dispatch to the wrapped data
        pub fn iter(&self) -> std::slice::Iter<TransactionSummary> {
            self.0.iter()
        }
        /// Total fees captured by these summaries
        pub fn paid_fees(&self) -> u64 {
            self.iter().filter_map(|summary| summary.fee()).sum()
        }
        /// A Vec of the txids
        pub fn txids(&self) -> Vec<TxId> {
            self.iter().map(|summary| summary.txid()).collect()
        }
    }

    impl std::fmt::Display for TransactionSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for transaction_summary in &self.0 {
                write!(f, "\n{}", transaction_summary)?;
            }
            Ok(())
        }
    }

    impl From<TransactionSummaries> for JsonValue {
        fn from(transaction_summaries: TransactionSummaries) -> Self {
            let transaction_summaries: Vec<JsonValue> = transaction_summaries
                .0
                .into_iter()
                .map(JsonValue::from)
                .collect();
            json::object! {
                "transaction_summaries" => transaction_summaries
            }
        }
    }

    /// Builds TransactionSummary from builder
    pub struct TransactionSummaryBuilder {
        txid: Option<TxId>,
        datetime: Option<u64>,
        status: Option<ConfirmationStatus>,
        blockheight: Option<BlockHeight>,
        kind: Option<TransactionKind>,
        value: Option<u64>,
        fee: Option<Option<u64>>,
        zec_price: Option<Option<f64>>,
        orchard_notes: Option<Vec<OrchardNoteSummary>>,
        sapling_notes: Option<Vec<SaplingNoteSummary>>,
        transparent_coins: Option<Vec<TransparentCoinSummary>>,
        outgoing_tx_data: Option<Vec<OutgoingTxData>>,
    }

    impl TransactionSummaryBuilder {
        /// Creates a new TransactionSummary builder
        pub fn new() -> TransactionSummaryBuilder {
            TransactionSummaryBuilder {
                txid: None,
                datetime: None,
                status: None,
                blockheight: None,
                kind: None,
                value: None,
                fee: None,
                zec_price: None,
                orchard_notes: None,
                sapling_notes: None,
                transparent_coins: None,
                outgoing_tx_data: None,
            }
        }

        build_method!(txid, TxId);
        build_method!(datetime, u64);
        build_method!(status, ConfirmationStatus);
        build_method!(blockheight, BlockHeight);
        build_method!(kind, TransactionKind);
        build_method!(value, u64);
        build_method!(fee, Option<u64>);
        build_method!(zec_price, Option<f64>);
        build_method!(orchard_notes, Vec<OrchardNoteSummary>);
        build_method!(sapling_notes, Vec<SaplingNoteSummary>);
        build_method!(transparent_coins, Vec<TransparentCoinSummary>);
        build_method!(outgoing_tx_data, Vec<OutgoingTxData>);

        /// Builds a TransactionSummary from builder
        pub fn build(&self) -> Result<TransactionSummary, BuildError> {
            Ok(TransactionSummary {
                txid: self
                    .txid
                    .ok_or(BuildError::MissingField("txid".to_string()))?,
                datetime: self
                    .datetime
                    .ok_or(BuildError::MissingField("datetime".to_string()))?,
                status: self
                    .status
                    .ok_or(BuildError::MissingField("status".to_string()))?,
                blockheight: self
                    .blockheight
                    .ok_or(BuildError::MissingField("blockheight".to_string()))?,
                kind: self
                    .kind
                    .ok_or(BuildError::MissingField("kind".to_string()))?,
                value: self
                    .value
                    .ok_or(BuildError::MissingField("value".to_string()))?,
                fee: self
                    .fee
                    .ok_or(BuildError::MissingField("fee".to_string()))?,
                zec_price: self
                    .zec_price
                    .ok_or(BuildError::MissingField("zec_price".to_string()))?,
                orchard_notes: self
                    .orchard_notes
                    .clone()
                    .ok_or(BuildError::MissingField("orchard_notes".to_string()))?,
                sapling_notes: self
                    .sapling_notes
                    .clone()
                    .ok_or(BuildError::MissingField("sapling_notes".to_string()))?,
                transparent_coins: self
                    .transparent_coins
                    .clone()
                    .ok_or(BuildError::MissingField("transparent_coins".to_string()))?,
                outgoing_tx_data: self
                    .outgoing_tx_data
                    .clone()
                    .ok_or(BuildError::MissingField("outgoing_tx_data".to_string()))?,
            })
        }
    }

    impl Default for TransactionSummaryBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Detailed transaction summary.
    /// A struct designed for conveniently displaying information to the user or converting to JSON to pass through an FFI.
    /// A "snapshot" of the state of a transaction in the wallet at the time the summary was constructed.
    /// Not to be used for internal logic in the system.
    #[derive(Clone, PartialEq, Debug)]
    pub struct DetailedTransactionSummary {
        txid: TxId,
        datetime: u64,
        status: ConfirmationStatus,
        blockheight: BlockHeight,
        kind: TransactionKind,
        value: u64,
        fee: Option<u64>,
        zec_price: Option<f64>,
        orchard_nullifiers: Vec<String>,
        sapling_nullifiers: Vec<String>,
        orchard_notes: Vec<OrchardNoteSummary>,
        sapling_notes: Vec<SaplingNoteSummary>,
        transparent_coins: Vec<TransparentCoinSummary>,
        outgoing_tx_data: Vec<OutgoingTxData>,
    }

    impl DetailedTransactionSummary {
        /// Gets orchard nullifiers
        pub fn orchard_nullifiers(&self) -> Vec<&str> {
            self.orchard_nullifiers.iter().map(|n| n.as_str()).collect()
        }
        /// Gets sapling nullifiers
        pub fn sapling_nullifiers(&self) -> Vec<&str> {
            self.sapling_nullifiers.iter().map(|n| n.as_str()).collect()
        }
    }

    impl TransactionSummaryInterface for DetailedTransactionSummary {
        fn txid(&self) -> TxId {
            self.txid
        }
        fn datetime(&self) -> u64 {
            self.datetime
        }
        fn status(&self) -> ConfirmationStatus {
            self.status
        }
        fn blockheight(&self) -> BlockHeight {
            self.blockheight
        }
        fn kind(&self) -> TransactionKind {
            self.kind
        }
        fn value(&self) -> u64 {
            self.value
        }
        fn fee(&self) -> Option<u64> {
            self.fee
        }
        fn zec_price(&self) -> Option<f64> {
            self.zec_price
        }
        fn orchard_notes(&self) -> &[OrchardNoteSummary] {
            &self.orchard_notes
        }
        fn sapling_notes(&self) -> &[SaplingNoteSummary] {
            &self.sapling_notes
        }
        fn transparent_coins(&self) -> &[TransparentCoinSummary] {
            &self.transparent_coins
        }
        fn outgoing_tx_data(&self) -> &[OutgoingTxData] {
            &self.outgoing_tx_data
        }
    }

    impl std::fmt::Display for DetailedTransactionSummary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let (
                datetime,
                fee,
                zec_price,
                orchard_notes,
                sapling_notes,
                transparent_coins,
                outgoing_tx_data_summaries,
            ) = self.prepare_for_display();
            let orchard_nullifier_summaries =
                OrchardNullifierSummaries(self.orchard_nullifiers.clone());
            let sapling_nullifier_summaries =
                SaplingNullifierSummaries(self.sapling_nullifiers.clone());
            write!(
                f,
                "{{
    txid: {}
    datetime: {}
    status: {}
    blockheight: {}
    kind: {}
    value: {}
    fee: {}
    zec price: {}
    orchard_nullifiers: {}
    sapling_nullifiers: {}
    orchard notes: {}
    sapling notes: {}
    transparent coins: {}
    outgoing data: {}
}}",
                self.txid,
                datetime,
                self.status,
                u64::from(self.blockheight),
                self.kind,
                self.value,
                fee,
                zec_price,
                orchard_nullifier_summaries,
                sapling_nullifier_summaries,
                orchard_notes,
                sapling_notes,
                transparent_coins,
                outgoing_tx_data_summaries,
            )
        }
    }

    impl From<DetailedTransactionSummary> for JsonValue {
        fn from(transaction: DetailedTransactionSummary) -> Self {
            json::object! {
                "txid" => transaction.txid.to_string(),
                "datetime" => transaction.datetime,
                "status" => transaction.status.to_string(),
                "blockheight" => u64::from(transaction.blockheight),
                "kind" => transaction.kind.to_string(),
                "value" => transaction.value,
                "fee" => transaction.fee,
                "zec_price" => transaction.zec_price,
                "orchard_nullifiers" => JsonValue::from(transaction.orchard_nullifiers),
                "sapling_nullifiers" => JsonValue::from(transaction.sapling_nullifiers),
                "orchard_notes" => JsonValue::from(transaction.orchard_notes),
                "sapling_notes" => JsonValue::from(transaction.sapling_notes),
                "transparent_coins" => JsonValue::from(transaction.transparent_coins),
                "outgoing_tx_data" => JsonValue::from(transaction.outgoing_tx_data),
            }
        }
    }

    /// Wraps a vec of detailed  transaction summaries for the implementation of std::fmt::Display
    #[derive(PartialEq, Debug)]
    pub struct DetailedTransactionSummaries(pub Vec<DetailedTransactionSummary>);

    impl DetailedTransactionSummaries {
        /// Creates a new Detailedtransactionsummaries struct
        pub fn new(transaction_summaries: Vec<DetailedTransactionSummary>) -> Self {
            DetailedTransactionSummaries(transaction_summaries)
        }
        /// Implicitly dispatch to the wrapped data
        pub fn iter(&self) -> std::slice::Iter<DetailedTransactionSummary> {
            self.0.iter()
        }
        /// Total fees captured by these summaries
        pub fn paid_fees(&self) -> u64 {
            self.iter().filter_map(|summary| summary.fee()).sum()
        }
        /// A Vec of the txids
        pub fn txids(&self) -> Vec<TxId> {
            self.iter().map(|summary| summary.txid()).collect()
        }
    }

    impl std::fmt::Display for DetailedTransactionSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for transaction_summary in &self.0 {
                write!(f, "\n{}", transaction_summary)?;
            }
            Ok(())
        }
    }

    impl From<DetailedTransactionSummaries> for JsonValue {
        fn from(transaction_summaries: DetailedTransactionSummaries) -> Self {
            let transaction_summaries: Vec<JsonValue> = transaction_summaries
                .0
                .into_iter()
                .map(JsonValue::from)
                .collect();
            json::object! {
                "detailed_transaction_summaries" => transaction_summaries
            }
        }
    }

    /// Builder for DetailedTransactionSummary
    pub struct DetailedTransactionSummaryBuilder {
        txid: Option<TxId>,
        datetime: Option<u64>,
        status: Option<ConfirmationStatus>,
        blockheight: Option<BlockHeight>,
        kind: Option<TransactionKind>,
        value: Option<u64>,
        fee: Option<Option<u64>>,
        zec_price: Option<Option<f64>>,
        orchard_nullifiers: Option<Vec<String>>,
        sapling_nullifiers: Option<Vec<String>>,
        orchard_notes: Option<Vec<OrchardNoteSummary>>,
        sapling_notes: Option<Vec<SaplingNoteSummary>>,
        transparent_coins: Option<Vec<TransparentCoinSummary>>,
        outgoing_tx_data: Option<Vec<OutgoingTxData>>,
    }

    impl DetailedTransactionSummaryBuilder {
        /// Creates a new DetailedTransactionSummary builder
        pub fn new() -> DetailedTransactionSummaryBuilder {
            DetailedTransactionSummaryBuilder {
                txid: None,
                datetime: None,
                status: None,
                blockheight: None,
                kind: None,
                value: None,
                fee: None,
                zec_price: None,
                orchard_nullifiers: None,
                sapling_nullifiers: None,
                orchard_notes: None,
                sapling_notes: None,
                transparent_coins: None,
                outgoing_tx_data: None,
            }
        }

        build_method!(txid, TxId);
        build_method!(datetime, u64);
        build_method!(status, ConfirmationStatus);
        build_method!(blockheight, BlockHeight);
        build_method!(kind, TransactionKind);
        build_method!(value, u64);
        build_method!(fee, Option<u64>);
        build_method!(zec_price, Option<f64>);
        build_method!(orchard_nullifiers, Vec<String>);
        build_method!(sapling_nullifiers, Vec<String>);
        build_method!(orchard_notes, Vec<OrchardNoteSummary>);
        build_method!(sapling_notes, Vec<SaplingNoteSummary>);
        build_method!(transparent_coins, Vec<TransparentCoinSummary>);
        build_method!(outgoing_tx_data, Vec<OutgoingTxData>);

        /// Builds DetailedTransactionSummary from builder
        pub fn build(&self) -> Result<DetailedTransactionSummary, BuildError> {
            Ok(DetailedTransactionSummary {
                txid: self
                    .txid
                    .ok_or(BuildError::MissingField("txid".to_string()))?,
                datetime: self
                    .datetime
                    .ok_or(BuildError::MissingField("datetime".to_string()))?,
                status: self
                    .status
                    .ok_or(BuildError::MissingField("status".to_string()))?,
                blockheight: self
                    .blockheight
                    .ok_or(BuildError::MissingField("blockheight".to_string()))?,
                kind: self
                    .kind
                    .ok_or(BuildError::MissingField("kind".to_string()))?,
                value: self
                    .value
                    .ok_or(BuildError::MissingField("value".to_string()))?,
                fee: self
                    .fee
                    .ok_or(BuildError::MissingField("fee".to_string()))?,
                zec_price: self
                    .zec_price
                    .ok_or(BuildError::MissingField("zec_price".to_string()))?,
                orchard_nullifiers: self
                    .orchard_nullifiers
                    .clone()
                    .ok_or(BuildError::MissingField("orchard_nullifiers".to_string()))?,
                sapling_nullifiers: self
                    .sapling_nullifiers
                    .clone()
                    .ok_or(BuildError::MissingField("sapling_nullifiers".to_string()))?,
                orchard_notes: self
                    .orchard_notes
                    .clone()
                    .ok_or(BuildError::MissingField("orchard_notes".to_string()))?,
                sapling_notes: self
                    .sapling_notes
                    .clone()
                    .ok_or(BuildError::MissingField("sapling_notes".to_string()))?,
                transparent_coins: self
                    .transparent_coins
                    .clone()
                    .ok_or(BuildError::MissingField("transparent_coins".to_string()))?,
                outgoing_tx_data: self
                    .outgoing_tx_data
                    .clone()
                    .ok_or(BuildError::MissingField("outgoing_tx_data".to_string()))?,
            })
        }
    }

    impl Default for DetailedTransactionSummaryBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Orchard note summary.
    /// A struct designed for conveniently displaying information to the user or converting to JSON to pass through an FFI.
    /// A "snapshot" of the state of the output in the wallet at the time the summary was constructed.
    /// Not to be used for internal logic in the system.
    #[derive(Clone, PartialEq, Debug)]
    pub struct OrchardNoteSummary {
        value: u64,
        spend_summary: SpendSummary,
        output_index: Option<u32>,
        memo: Option<String>,
    }

    impl OrchardNoteSummary {
        /// Creates an OrchardNoteSummary from parts
        pub fn from_parts(
            value: u64,
            spend_status: SpendSummary,
            output_index: Option<u32>,
            memo: Option<String>,
        ) -> Self {
            OrchardNoteSummary {
                value,
                spend_summary: spend_status,
                output_index,
                memo,
            }
        }
        /// Gets value
        pub fn value(&self) -> u64 {
            self.value
        }

        /// Gets spend status
        pub fn spend_summary(&self) -> SpendSummary {
            self.spend_summary
        }

        /// Gets output index
        pub fn output_index(&self) -> Option<u32> {
            self.output_index
        }
        /// Gets memo
        pub fn memo(&self) -> Option<&str> {
            self.memo.as_deref()
        }
    }

    impl std::fmt::Display for OrchardNoteSummary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let output_index = if let Some(i) = self.output_index {
                i.to_string()
            } else {
                "not available".to_string()
            };
            let memo = if let Some(m) = self.memo.clone() {
                m
            } else {
                "".to_string()
            };
            write!(
                f,
                "\t{{
            value: {}
            spend status: {}
            output index: {}
            memo: {}
        }}",
                self.value, self.spend_summary, output_index, memo,
            )
        }
    }

    impl From<OrchardNoteSummary> for JsonValue {
        fn from(note: OrchardNoteSummary) -> Self {
            json::object! {
                "value" => note.value,
                "spend_status" => note.spend_summary.to_string(),
                "output_index" => note.output_index,
                "memo" => note.memo,
            }
        }
    }

    /// Wraps a vec of orchard note summaries for the implementation of std::fmt::Display
    pub struct OrchardNoteSummaries(Vec<OrchardNoteSummary>);

    impl std::fmt::Display for OrchardNoteSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for note in &self.0 {
                write!(f, "\n{}", note)?;
            }
            Ok(())
        }
    }

    /// Sapling note summary.
    /// A struct designed for conveniently displaying information to the user or converting to JSON to pass through an FFI.
    /// A "snapshot" of the state of the output in the wallet at the time the summary was constructed.
    /// Not to be used for internal logic in the system.
    #[derive(Clone, PartialEq, Debug)]
    pub struct SaplingNoteSummary {
        value: u64,
        spend_summary: SpendSummary,
        output_index: Option<u32>,
        memo: Option<String>,
    }

    impl SaplingNoteSummary {
        /// Creates a SaplingNoteSummary from parts
        pub fn from_parts(
            value: u64,
            spend_status: SpendSummary,
            output_index: Option<u32>,
            memo: Option<String>,
        ) -> Self {
            SaplingNoteSummary {
                value,
                spend_summary: spend_status,
                output_index,
                memo,
            }
        }
        /// Gets value
        pub fn value(&self) -> u64 {
            self.value
        }

        /// Gets spend status
        pub fn spend_summary(&self) -> SpendSummary {
            self.spend_summary
        }

        /// Gets output index
        pub fn output_index(&self) -> Option<u32> {
            self.output_index
        }
        /// Gets memo
        pub fn memo(&self) -> Option<&str> {
            self.memo.as_deref()
        }
    }

    impl std::fmt::Display for SaplingNoteSummary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let output_index = if let Some(i) = self.output_index {
                i.to_string()
            } else {
                "not available".to_string()
            };
            let memo = if let Some(m) = self.memo.clone() {
                m
            } else {
                "no memo".to_string()
            };
            write!(
                f,
                "\t{{
            value: {}
            spend status: {}
            output index: {}
            memo: {}
        }}",
                self.value, self.spend_summary, output_index, memo,
            )
        }
    }

    impl From<SaplingNoteSummary> for JsonValue {
        fn from(note: SaplingNoteSummary) -> Self {
            json::object! {
                "value" => note.value,
                "spend_status" => note.spend_summary.to_string(),
                "output_index" => note.output_index,
                "memo" => note.memo,
            }
        }
    }

    /// Wraps a vec of sapling note summaries for the implementation of std::fmt::Display
    pub struct SaplingNoteSummaries(Vec<SaplingNoteSummary>);

    impl std::fmt::Display for SaplingNoteSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for note in &self.0 {
                write!(f, "\n{}", note)?;
            }
            Ok(())
        }
    }

    /// Transparent coin summary.
    /// A struct designed for conveniently displaying information to the user or converting to JSON to pass through an FFI.
    /// A "snapshot" of the state of the output in the wallet at the time the summary was constructed.
    /// Not to be used for internal logic in the system.
    #[derive(Clone, PartialEq, Debug)]
    pub struct TransparentCoinSummary {
        value: u64,
        spend_summary: SpendSummary,
        output_index: u64,
    }

    impl TransparentCoinSummary {
        /// Creates a SaplingNoteSummary from parts
        pub fn from_parts(value: u64, spend_status: SpendSummary, output_index: u64) -> Self {
            TransparentCoinSummary {
                value,
                spend_summary: spend_status,
                output_index,
            }
        }
        /// Gets value
        pub fn value(&self) -> u64 {
            self.value
        }

        /// Gets spend status
        pub fn spend_summary(&self) -> SpendSummary {
            self.spend_summary
        }

        /// Gets output index
        pub fn output_index(&self) -> u64 {
            self.output_index
        }
    }

    impl std::fmt::Display for TransparentCoinSummary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "\t{{
            value: {}
            spend status: {}
            output index: {}
        }}",
                self.value, self.spend_summary, self.output_index,
            )
        }
    }
    impl From<TransparentCoinSummary> for JsonValue {
        fn from(note: TransparentCoinSummary) -> Self {
            json::object! {
                "value" => note.value,
                "spend_status" => note.spend_summary.to_string(),
                "output_index" => note.output_index,
            }
        }
    }

    /// Wraps a vec of transparent coin summaries for the implementation of std::fmt::Display
    pub struct TransparentCoinSummaries(Vec<TransparentCoinSummary>);

    impl std::fmt::Display for TransparentCoinSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for coin in &self.0 {
                write!(f, "\n{}", coin)?;
            }
            Ok(())
        }
    }

    /// Wraps a vec of orchard nullifier summaries for the implementation of std::fmt::Display
    pub struct OrchardNullifierSummaries(Vec<String>);

    impl std::fmt::Display for OrchardNullifierSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for nullifier in &self.0 {
                write!(f, "\n{}", nullifier)?;
            }
            Ok(())
        }
    }

    /// Wraps a vec of sapling nullifier summaries for the implementation of std::fmt::Display
    struct SaplingNullifierSummaries(Vec<String>);

    impl std::fmt::Display for SaplingNullifierSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for nullifier in &self.0 {
                write!(f, "\n{}", nullifier)?;
            }
            Ok(())
        }
    }

    /// Spend status of an output
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub enum SpendSummary {
        /// Output is not spent.
        Unspent,
        /// Output is pending spent.
        /// The transaction consuming this output has been transmitted.
        TransmittedSpent(TxId),
        /// Output is pending spent.
        /// The transaction consuming this output has been detected in the mempool.
        MempoolSpent(TxId),
        /// Output is spent.
        /// The transaction consuming this output is confirmed.
        Spent(TxId),
    }

    impl SpendSummary {
        /// converts the interface spend to a SpendSummary
        pub fn from_spend(spend: &Option<(TxId, ConfirmationStatus)>) -> Self {
            match spend {
                Some((txid, ConfirmationStatus::Transmitted(_))) => {
                    SpendSummary::TransmittedSpent(*txid)
                }

                Some((txid, ConfirmationStatus::Mempool(_))) => SpendSummary::MempoolSpent(*txid),
                Some((txid, ConfirmationStatus::Confirmed(_))) => SpendSummary::Spent(*txid),
                _ => SpendSummary::Unspent,
            }
        }
    }

    impl std::fmt::Display for SpendSummary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                SpendSummary::Unspent => write!(f, "unspent"),
                SpendSummary::TransmittedSpent(txid) => write!(f, "transmitted: spent in {}", txid),
                SpendSummary::Spent(txid) => write!(f, "confirmed: spent in {}", txid),
                SpendSummary::MempoolSpent(txid) => write!(f, "mempool: spent in {}", txid),
            }
        }
    }

    pub(crate) fn basic_transaction_summary_parts(
        transaction_record: &TransactionRecord,
        transaction_records: &TransactionRecordsById,
        chain: &ChainType,
    ) -> (
        TransactionKind,
        u64,
        Option<u64>,
        Vec<OrchardNoteSummary>,
        Vec<SaplingNoteSummary>,
        Vec<TransparentCoinSummary>,
    ) {
        let kind = transaction_records.transaction_kind(transaction_record, chain);
        let value = match kind {
            TransactionKind::Received
            | TransactionKind::Sent(SendType::Shield)
            | TransactionKind::Sent(SendType::SendToSelf) => {
                transaction_record.total_value_received()
            }
            TransactionKind::Sent(SendType::Send) => transaction_record.value_outgoing(),
        };
        let fee = transaction_records
            .calculate_transaction_fee(transaction_record)
            .ok();
        let mut orchard_notes = transaction_record
            .orchard_notes
            .iter()
            .map(|output| {
                let spend_summary = SpendSummary::from_spend(output.spending_tx_status());

                let memo = if let Some(Memo::Text(memo_text)) = &output.memo {
                    Some(memo_text.to_string())
                } else {
                    None
                };

                OrchardNoteSummary::from_parts(
                    output.value(),
                    spend_summary,
                    output.output_index,
                    memo,
                )
            })
            .collect::<Vec<_>>();
        let mut sapling_notes = transaction_record
            .sapling_notes
            .iter()
            .map(|output| {
                let spend_summary = SpendSummary::from_spend(output.spending_tx_status());

                let memo = if let Some(Memo::Text(memo_text)) = &output.memo {
                    Some(memo_text.to_string())
                } else {
                    None
                };

                SaplingNoteSummary::from_parts(
                    output.value(),
                    spend_summary,
                    output.output_index,
                    memo,
                )
            })
            .collect::<Vec<_>>();
        let mut transparent_coins = transaction_record
            .transparent_outputs
            .iter()
            .map(|output| {
                let spend_summary = SpendSummary::from_spend(output.spending_tx_status());

                TransparentCoinSummary::from_parts(
                    output.value(),
                    spend_summary,
                    output.output_index,
                )
            })
            .collect::<Vec<_>>();

        // TODO: this sorting should be removed once we root cause the tx records outputs being out of order
        orchard_notes.sort_by_key(|output| output.output_index());
        sapling_notes.sort_by_key(|output| output.output_index());
        transparent_coins.sort_by_key(|output| output.output_index());
        (
            kind,
            value,
            fee,
            orchard_notes,
            sapling_notes,
            transparent_coins,
        )
    }
}

/// Convenience wrapper for primitive
#[derive(Debug)]
pub struct SpendableSaplingNote {
    /// TODO: Add Doc Comment Here!
    pub transaction_id: TxId,
    /// TODO: Add Doc Comment Here!
    pub nullifier: sapling_crypto::Nullifier,
    /// TODO: Add Doc Comment Here!
    pub diversifier: sapling_crypto::Diversifier,
    /// TODO: Add Doc Comment Here!
    pub note: sapling_crypto::Note,
    /// TODO: Add Doc Comment Here!
    pub witnessed_position: Position,
    /// TODO: Add Doc Comment Here!
    pub extsk: Option<sapling_crypto::zip32::ExtendedSpendingKey>,
}

/// TODO: Add Doc Comment Here!
#[derive(Debug)]
pub struct SpendableOrchardNote {
    /// TODO: Add Doc Comment Here!
    pub transaction_id: TxId,
    /// TODO: Add Doc Comment Here!
    pub nullifier: orchard::note::Nullifier,
    /// TODO: Add Doc Comment Here!
    pub diversifier: orchard::keys::Diversifier,
    /// TODO: Add Doc Comment Here!
    pub note: orchard::note::Note,
    /// TODO: Add Doc Comment Here!
    pub witnessed_position: Position,
    /// TODO: Add Doc Comment Here!
    pub spend_key: Option<orchard::keys::SpendingKey>,
}

/// Struct that tracks the latest and historical price of ZEC in the wallet
#[derive(Clone, Debug)]
pub struct WalletZecPriceInfo {
    /// Latest price of ZEC and when it was fetched
    pub zec_price: Option<(u64, f64)>,

    /// Wallet's currency. All the prices are in this currency
    pub currency: String,

    /// When the last time historical prices were fetched
    pub last_historical_prices_fetched_at: Option<u64>,

    /// Historical prices retry count
    pub historical_prices_retry_count: u64,
}

impl Default for WalletZecPriceInfo {
    fn default() -> Self {
        Self {
            zec_price: None,
            currency: "USD".to_string(), // Only USD is supported right now.
            last_historical_prices_fetched_at: None,
            historical_prices_retry_count: 0,
        }
    }
}

impl WalletZecPriceInfo {
    /// TODO: Add Doc Comment Here!
    pub fn serialized_version() -> u64 {
        20
    }

    /// TODO: Add Doc Comment Here!
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

    /// TODO: Add Doc Comment Here!
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

/// Generate a new rejection address,
/// for use in a send to a TEX address.
pub fn new_rejection_address(
    rejection_addresses: &append_only_vec::AppendOnlyVec<(
        TransparentAddress,
        TransparentAddressMetadata,
    )>,

    rejection_ivk: &zcash_primitives::legacy::keys::EphemeralIvk,
) -> Result<
    (
        zcash_primitives::legacy::TransparentAddress,
        zcash_client_backend::wallet::TransparentAddressMetadata,
    ),
    super::error::KeyError,
> {
    let (rejection_address, metadata) =
        super::keys::unified::WalletCapability::get_rejection_address_by_index(
            rejection_ivk,
            rejection_addresses.len() as u32,
        )?;
    rejection_addresses.push((rejection_address, metadata.clone()));
    Ok((rejection_address, metadata))
}

#[test]
fn read_write_empty_sapling_tree() {
    let mut buffer = Vec::new();

    write_commitment_tree(
        &CommitmentTree::<sapling_crypto::Node, 32>::empty(),
        &mut buffer,
    )
    .unwrap();
    assert_eq!(
        CommitmentTree::<sapling_crypto::Node, 32>::empty(),
        read_commitment_tree(&mut buffer.as_slice()).unwrap()
    )
}

#[cfg(any(test, feature = "test-elevation"))]
pub(crate) mod mocks {
    use zcash_primitives::memo::Memo;

    use crate::utils::build_method;

    use super::OutgoingTxData;

    pub(crate) struct OutgoingTxDataBuilder {
        recipient_address: Option<String>,
        value: Option<u64>,
        memo: Option<Memo>,
        recipient_ua: Option<Option<String>>,
        output_index: Option<Option<u64>>,
    }

    impl OutgoingTxDataBuilder {
        pub(crate) fn new() -> Self {
            Self {
                recipient_address: None,
                value: None,
                memo: None,
                recipient_ua: None,
                output_index: None,
            }
        }

        // Methods to set each field
        build_method!(recipient_address, String);
        build_method!(value, u64);
        build_method!(memo, Memo);
        build_method!(recipient_ua, Option<String>);
        build_method!(output_index, Option<u64>);

        pub(crate) fn build(&self) -> OutgoingTxData {
            OutgoingTxData {
                recipient_address: self.recipient_address.clone().unwrap(),
                value: self.value.unwrap(),
                memo: self.memo.clone().unwrap(),
                recipient_ua: self.recipient_ua.clone().unwrap(),
                output_index: self.output_index.unwrap(),
            }
        }
    }

    impl Default for OutgoingTxDataBuilder {
        fn default() -> Self {
            let mut builder = Self::new();
            builder
                .recipient_address("default_address".to_string())
                .value(50_000)
                .memo(Memo::default())
                .recipient_ua(None)
                .output_index(None);

            builder
        }
    }
}

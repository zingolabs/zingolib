//! TODO: Add Mod Description Here!
use super::traits::ToBytes;

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::frontier::CommitmentTree;
use incrementalmerkletree::witness::IncrementalWitness;
use incrementalmerkletree::{Hashable, Position};

use prost::Message;

use std::convert::TryFrom;
use std::io::{self, Read, Write};
use zcash_client_backend::proto::compact_formats::CompactBlock;

use zcash_encoding::{Optional, Vector};

use zcash_primitives::memo::MemoBytes;
use zcash_primitives::merkle_tree::{read_commitment_tree, write_commitment_tree};
use zcash_primitives::{memo::Memo, transaction::TxId};

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
}
impl std::fmt::Display for OutgoingTxData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Format the recipient address or unified address if provided.
        let address_display = if let Some(ref ua) = self.recipient_ua {
            format!("Unified Address: {}", ua)
        } else {
            format!("Recipient Address: {}", self.recipient_address)
        };
        let memo_text = if let Memo::Text(mt) = self.memo.clone() {
            mt.to_string()
        } else {
            "not a text memo".to_string()
        };

        write!(
            f,
            "{}\nValue: {}\nMemo: {}",
            address_display, self.value, memo_text
        )
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
    /// TODO: Add Doc Comment Here!
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

        Ok(OutgoingTxData {
            recipient_address: address,
            value,
            memo,
            recipient_ua: None,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
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
        writer.write_all(self.memo.encode().as_array())
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

/// TODO: Add Mod Description Here!
pub mod summaries {
    use json::{object, JsonValue};
    use zcash_client_backend::PoolType;
    use zcash_primitives::transaction::TxId;

    /// The MobileTx is the zingolib representation of
    /// transactions in the format most useful for
    /// consumption in mobile and mobile-like UI
    #[derive(PartialEq)]
    pub struct ValueTransfer {
        /// TODO: Add Doc Comment Here!
        pub block_height: zcash_primitives::consensus::BlockHeight,
        /// TODO: Add Doc Comment Here!
        pub datetime: u64,
        /// TODO: Add Doc Comment Here!
        pub kind: ValueTransferKind,
        /// TODO: Add Doc Comment Here!
        pub memos: Vec<zcash_primitives::memo::TextMemo>,
        /// TODO: Add Doc Comment Here!
        pub price: Option<f64>,
        /// TODO: Add Doc Comment Here!
        pub txid: TxId,
        /// TODO: Add Doc Comment Here!
        pub pending: bool,
    }

    impl ValueTransfer {
        /// Depending on the relationship of this Capability to the
        /// receiver capability assign polarity to amount transferred.
        pub fn balance_delta(&self) -> i64 {
            match self.kind {
                ValueTransferKind::Sent { amount, .. } => -(amount as i64),
                ValueTransferKind::Fee { amount, .. } => -(amount as i64),
                ValueTransferKind::Received { amount, .. } => amount as i64,
                ValueTransferKind::SendToSelf => 0,
            }
        }
    }

    impl std::fmt::Debug for ValueTransfer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            use core::ops::Deref as _;
            f.debug_struct("ValueTransfer")
                .field("block_height", &self.block_height)
                .field("datetime", &self.datetime)
                .field("kind", &self.kind)
                .field(
                    "memos",
                    &self
                        .memos
                        .iter()
                        .map(zcash_primitives::memo::TextMemo::deref)
                        .collect::<Vec<_>>(),
                )
                .field("price", &self.price)
                .field("txid", &self.txid)
                .field("pending", &self.pending)
                .finish()
        }
    }

    /// TODO: Add Doc Comment Here!
    #[derive(Clone, PartialEq, Eq, Debug)]
    pub enum ValueTransferKind {
        /// TODO: Add Doc Comment Here!
        Sent {
            /// TODO: Add Doc Comment Here!
            amount: u64,
            /// TODO: Add Doc Comment Here!
            recipient_address: zcash_address::ZcashAddress,
        },
        /// TODO: Add Doc Comment Here!
        Received {
            /// TODO: Add Doc Comment Here!
            pool_type: PoolType,
            /// TODO: Add Doc Comment Here!
            amount: u64,
        },
        /// TODO: Add Doc Comment Here!
        SendToSelf,
        /// TODO: Add Doc Comment Here!
        Fee {
            /// TODO: Add Doc Comment Here!
            amount: u64,
        },
    }

    impl From<&ValueTransferKind> for JsonValue {
        fn from(value: &ValueTransferKind) -> Self {
            match value {
                ValueTransferKind::Sent { .. } => JsonValue::String(String::from("Sent")),
                ValueTransferKind::Received { .. } => JsonValue::String(String::from("Received")),
                ValueTransferKind::SendToSelf => JsonValue::String(String::from("SendToSelf")),
                ValueTransferKind::Fee { .. } => JsonValue::String(String::from("Fee")),
            }
        }
    }

    impl From<ValueTransfer> for JsonValue {
        fn from(value: ValueTransfer) -> Self {
            let mut temp_object = object! {
                    "amount": "",
                    "block_height": u32::from(value.block_height),
                    "datetime": value.datetime,
                    "kind": "",
                    "memos": value.memos.iter().cloned().map(String::from).collect::<Vec<String>>(),
                    "pool_type": "",
                    "price": value.price,
                    "txid": value.txid.to_string(),
                    "pending": value.pending,
            };
            match value.kind {
                ValueTransferKind::Sent {
                    ref recipient_address,
                    amount,
                } => {
                    temp_object["amount"] = JsonValue::from(amount);
                    temp_object["kind"] = JsonValue::from(&value.kind);
                    temp_object["to_address"] = JsonValue::from(recipient_address.encode());
                    temp_object
                }
                ValueTransferKind::Fee { amount } => {
                    temp_object["amount"] = JsonValue::from(amount);
                    temp_object["kind"] = JsonValue::from(&value.kind);
                    temp_object
                }
                ValueTransferKind::Received { pool_type, amount } => {
                    temp_object["amount"] = JsonValue::from(amount);
                    temp_object["kind"] = JsonValue::from(&value.kind);
                    temp_object["pool_type"] = JsonValue::from(pool_type.to_string());
                    temp_object
                }
                ValueTransferKind::SendToSelf => {
                    temp_object["amount"] = JsonValue::from(0);
                    temp_object["kind"] = JsonValue::from(&value.kind);
                    temp_object["pool"] = JsonValue::from("None".to_string());
                    temp_object["price"] = JsonValue::from("None".to_string());
                    temp_object["to_address"] = JsonValue::from("None".to_string());
                    temp_object
                }
            }
        }
    }
}

pub use crate::wallet::transaction_record::TransactionRecord;

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

#[cfg(test)]
pub(crate) mod mocks {
    use zcash_primitives::memo::Memo;

    use crate::mocks::build_method;

    use super::OutgoingTxData;

    pub(crate) struct OutgoingTxDataBuilder {
        recipient_address: Option<String>,
        value: Option<u64>,
        memo: Option<Memo>,
        recipient_ua: Option<Option<String>>,
    }

    impl OutgoingTxDataBuilder {
        pub(crate) fn new() -> Self {
            Self {
                recipient_address: None,
                value: None,
                memo: None,
                recipient_ua: None,
            }
        }

        // Methods to set each field
        build_method!(recipient_address, String);
        build_method!(value, u64);
        build_method!(memo, Memo);
        build_method!(recipient_ua, Option<String>);

        pub(crate) fn build(&self) -> OutgoingTxData {
            OutgoingTxData {
                recipient_address: self.recipient_address.clone().unwrap(),
                value: self.value.unwrap(),
                memo: self.memo.clone().unwrap(),
                recipient_ua: self.recipient_ua.clone().unwrap(),
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
                .recipient_ua(None);
            builder
        }
    }
}

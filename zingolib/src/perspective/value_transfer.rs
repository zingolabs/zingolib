//! The value-transfer editorial types: per-recipient statements of value
//! movement derived from transaction summaries.

use chrono::DateTime;
use json::JsonValue;

use zcash_protocol::{PoolType, TxId, consensus::BlockHeight};

use crate::wallet::summary::data::{TransactionSummary, display_pools, pools_to_json};
use zingo_status::confirmation_status::ConfirmationStatus;

/// Value transfer kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValueTransferKind {
    /// Sent value transfer.
    Sent(SentValueTransfer),
    /// Received value transfer.
    Received,
}

/// Sent value transfer kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SentValueTransfer {
    /// Transferring funds to an address that is not derived by the wallet.
    Send,
    /// Transferring funds to an address that is derived by the wallet.
    SendToSelf(SelfSendValueTransfer),
}

/// Send-to-self value transfer kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelfSendValueTransfer {
    /// No memo.
    ///
    /// Only occurs when there are no other value transfers created for a given transaction.
    Basic,
    /// Shielding transparent funds to a shielded pool.
    Shield,
    /// Sending memo to a wallet's own address.
    MemoToSelf,
    /// Transferring funds from a shielded pool to one of the wallet's own refund (ephemeral) addresses as the
    /// first step in a TEX transaction.
    Refund,
    /// Migrating funds from the Orchard pool into the wallet's own Ironwood pool
    /// (an Orchard -> Ironwood self-send), as part of the NU6.3 migration.
    Migration,
}

impl std::fmt::Display for ValueTransferKind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ValueTransferKind::Received => write!(f, "received"),
            ValueTransferKind::Sent(sent) => match sent {
                SentValueTransfer::Send => write!(f, "sent"),
                SentValueTransfer::SendToSelf(selfsend) => match selfsend {
                    SelfSendValueTransfer::Basic => write!(f, "send-to-self"),
                    SelfSendValueTransfer::Shield => write!(f, "shield"),
                    SelfSendValueTransfer::MemoToSelf => write!(f, "memo-to-self"),
                    SelfSendValueTransfer::Refund => write!(f, "refund"),
                    SelfSendValueTransfer::Migration => write!(f, "migration"),
                },
            },
        }
    }
}

/// A value transfer is a note group abstraction.
/// A group of all notes sent to a specific address in a transaction.
#[allow(missing_docs)]
#[derive(Clone, PartialEq)]
pub struct ValueTransfer {
    pub txid: TxId,
    pub datetime: u32,
    pub status: ConfirmationStatus,
    pub blockheight: BlockHeight,
    pub transaction_fee: Option<u64>,
    pub zec_price: Option<f32>,
    pub kind: ValueTransferKind,
    pub value: u64,
    pub recipient_address: Option<String>,
    /// Pools of this wallet's outputs spent to fund the transaction this value transfer
    /// belongs to. Transaction-level: the same for every value transfer of a txid, and
    /// empty for received transactions.
    ///
    /// Together with [`Self::pools_received`] this exposes pool movement. An
    /// Orchard -> Ironwood send-to-self (`pools_sent_from: [Orchard]`,
    /// `pools_received: [Ironwood]`) is classified as
    /// [`SelfSendValueTransfer::Migration`] by the editorial layer itself. Interpreting any
    /// other pool movement is left to the consumer.
    pub pools_sent_from: Vec<PoolType>,
    /// Pools this value transfer's value arrived into: the pool of the grouped notes for
    /// received and shielding transfers, the pools of the recipient's outputs for sent
    /// transfers, and the pools of all self-received outputs for send-to-self transfers.
    pub pools_received: Vec<PoolType>,
    pub memos: Vec<String>,
}

impl ValueTransfer {
    /// Builds a value transfer, carrying over the transaction-level fields
    /// (txid, datetime, status, blockheight, fee, price, pools sent from)
    /// from `transaction`.
    pub(crate) fn from_summary(
        transaction: &TransactionSummary,
        kind: ValueTransferKind,
        value: u64,
        recipient_address: Option<String>,
        pools_received: Vec<PoolType>,
        memos: Vec<String>,
    ) -> Self {
        Self {
            txid: transaction.txid,
            datetime: transaction.datetime,
            status: transaction.status,
            blockheight: transaction.blockheight,
            transaction_fee: transaction.fee,
            zec_price: transaction.zec_price,
            kind,
            value,
            recipient_address,
            pools_sent_from: transaction.pools_sent_from.clone(),
            pools_received,
            memos,
        }
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
            .field("pools_sent_from", &self.pools_sent_from)
            .field("pools_received", &self.pools_received)
            .field("memos", &self.memos)
            .finish()
    }
}

impl std::fmt::Display for ValueTransfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let datetime = if let Some(dt) = DateTime::from_timestamp(i64::from(self.datetime), 0) {
            format!("{dt}")
        } else {
            "not available".to_string()
        };
        let transaction_fee = if let Some(f) = self.transaction_fee {
            f.to_string()
        } else {
            "not available".to_string()
        };
        let zec_price = if let Some(price) = self.zec_price {
            price.to_string()
        } else {
            "not available".to_string()
        };
        let recipient_address = if let Some(addr) = self.recipient_address.as_ref() {
            addr.clone()
        } else {
            "not available".to_string()
        };
        let mut memos = String::new();
        for (index, memo) in self.memos.iter().enumerate() {
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
    pools_sent_from: {}
    pools_received: {}
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
            display_pools(&self.pools_sent_from),
            display_pools(&self.pools_received),
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
            "pools_sent_from" => pools_to_json(&value_transfer.pools_sent_from),
            "pools_received" => pools_to_json(&value_transfer.pools_received),
            "memos" => value_transfer.memos
        }
    }
}

/// A wrapper struct for implementing display and json on a vec of value transfers
#[derive(PartialEq, Debug)]
pub struct ValueTransfers(Vec<ValueTransfer>);
impl<'a> std::iter::IntoIterator for &'a ValueTransfers {
    type Item = &'a ValueTransfer;
    type IntoIter = std::slice::Iter<'a, ValueTransfer>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl std::ops::Deref for ValueTransfers {
    type Target = Vec<ValueTransfer>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for ValueTransfers {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
// Implement the Index trait
impl std::ops::Index<usize> for ValueTransfers {
    type Output = ValueTransfer; // The type of the value returned by the index

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index] // Forward the indexing operation to the underlying data structure
    }
}

impl ValueTransfers {
    /// Creates a new `ValueTransfers`
    #[must_use]
    pub fn new(value_transfers: Vec<ValueTransfer>) -> Self {
        ValueTransfers(value_transfers)
    }
}

impl std::fmt::Display for ValueTransfers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for value_transfer in &self.0 {
            write!(f, "\n{value_transfer}")?;
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

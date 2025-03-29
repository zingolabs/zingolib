//! TODO: Add Mod Description Here!

use std::io::{self, Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use zcash_encoding::Optional;

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
// TODO: move to summary module and remove unecessary builders and bloat etc.
pub mod summaries {
    use std::collections::HashMap;

    use chrono::DateTime;
    use json::JsonValue;
    use pepper_sync::keys::KeyId;
    use zcash_primitives::{consensus::BlockHeight, transaction::TxId};
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::{
        error::BuildError,
        utils::build_method,
        wallet::{
            output::SpendStatus,
            summary::{SendType, TransactionKind},
        },
    };

    /// A value transfer is a note group abstraction.
    /// A group of all notes sent to a specific address in a transaction.
    #[derive(Clone, PartialEq)]
    pub struct ValueTransfer {
        txid: TxId,
        datetime: u32,
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
        pub fn datetime(&self) -> u32 {
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
        /// Creates a new ValueTransfer
        pub fn new(value_transfers: Vec<ValueTransfer>) -> Self {
            ValueTransfers(value_transfers)
        }

        /// Creates value transfers for all notes in a transaction that are sent to another
        /// recipient.  A value transfer is a group of all notes to a specific receiver in a transaction.
        /// The value transfer list is sorted by the output index of the notes.
        pub fn create_send_value_transfers(
            transaction_summary: &TransactionSummary,
        ) -> Vec<ValueTransfer> {
            let mut value_transfers: Vec<ValueTransfer> = Vec::new();
            let outgoing_notes = transaction_summary
                .outgoing_orchard_notes
                .iter()
                .chain(transaction_summary.outgoing_sapling_notes.iter())
                .collect::<Vec<_>>();
            let mut addresses = HashMap::with_capacity(outgoing_notes.len());
            outgoing_notes.iter().for_each(|note| {
                let address = if let Some(ua) = note.recipient_unified_address.clone() {
                    ua
                } else {
                    note.recipient.clone()
                };
                // hash map is used to create unique list of addresses as duplicates are not inserted twice
                addresses.insert(address, note.output_index);
            });
            let mut addresses_vec = addresses.into_iter().collect::<Vec<_>>();
            addresses_vec.sort_by_key(|(_address, output_index)| *output_index);
            addresses_vec.iter().for_each(|(address, _output_index)| {
                let outgoing_notes_to_address: Vec<&OutgoingNoteSummary> = outgoing_notes
                    .iter()
                    .filter(|&&note| {
                        let query_address = if let Some(ua) = note.recipient_unified_address.clone()
                        {
                            ua
                        } else {
                            note.recipient.clone()
                        };
                        query_address == *address
                    })
                    .cloned()
                    .collect();
                let value: u64 = outgoing_notes_to_address
                    .iter()
                    .map(|&note| note.value)
                    .sum();
                let memos: Vec<String> = outgoing_notes_to_address
                    .iter()
                    .filter_map(|&note| note.memo.clone())
                    .collect();
                value_transfers.push(
                    ValueTransferBuilder::new()
                        .txid(transaction_summary.txid())
                        .datetime(transaction_summary.datetime())
                        .status(transaction_summary.status())
                        .blockheight(transaction_summary.blockheight())
                        .transaction_fee(transaction_summary.fee())
                        .zec_price(transaction_summary.zec_price())
                        .kind(ValueTransferKind::Sent(SentValueTransfer::Send))
                        .value(value)
                        .recipient_address(Some(address.clone()))
                        .pool_received(None)
                        .memos(memos)
                        .build()
                        .expect("all fields should be populated"),
                );
            });
            value_transfers
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
        datetime: Option<u32>,
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
        build_method!(datetime, u32);
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
        fn datetime(&self) -> u32;
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
        fn orchard_notes(&self) -> &[BasicNoteSummary];
        /// Gets slice of sapling note summaries
        fn sapling_notes(&self) -> &[BasicNoteSummary];
        /// Gets slice of transparent coin summaries
        fn transparent_coins(&self) -> &[BasicCoinSummary];
        /// Gets slice of outgoing orchard notes
        fn outgoing_orchard_notes(&self) -> &[OutgoingNoteSummary];
        /// Gets slice of outgoing sapling notes
        fn outgoing_sapling_notes(&self) -> &[OutgoingNoteSummary];
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
            BasicNoteSummaries,
            BasicNoteSummaries,
            BasicCoinSummaries,
            OutgoingNoteSummaries,
            OutgoingNoteSummaries,
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
            let orchard_notes = BasicNoteSummaries(self.orchard_notes().to_vec());
            let sapling_notes = BasicNoteSummaries(self.sapling_notes().to_vec());
            let transparent_coins = BasicCoinSummaries(self.transparent_coins().to_vec());
            let outgoing_orchard_notes =
                OutgoingNoteSummaries(self.outgoing_orchard_notes().to_vec());
            let outgoing_sapling_notes =
                OutgoingNoteSummaries(self.outgoing_sapling_notes().to_vec());

            (
                datetime,
                fee,
                zec_price,
                orchard_notes,
                sapling_notes,
                transparent_coins,
                outgoing_orchard_notes,
                outgoing_sapling_notes,
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
        datetime: u32,
        status: ConfirmationStatus,
        blockheight: BlockHeight,
        kind: TransactionKind,
        value: u64,
        fee: Option<u64>,
        zec_price: Option<f64>,
        orchard_notes: Vec<BasicNoteSummary>,
        sapling_notes: Vec<BasicNoteSummary>,
        transparent_coins: Vec<BasicCoinSummary>,
        outgoing_orchard_notes: Vec<OutgoingNoteSummary>,
        outgoing_sapling_notes: Vec<OutgoingNoteSummary>,
    }

    impl TransactionSummaryInterface for TransactionSummary {
        fn txid(&self) -> TxId {
            self.txid
        }
        fn datetime(&self) -> u32 {
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
        fn orchard_notes(&self) -> &[BasicNoteSummary] {
            &self.orchard_notes
        }
        fn sapling_notes(&self) -> &[BasicNoteSummary] {
            &self.sapling_notes
        }
        fn transparent_coins(&self) -> &[BasicCoinSummary] {
            &self.transparent_coins
        }
        fn outgoing_orchard_notes(&self) -> &[OutgoingNoteSummary] {
            &self.outgoing_orchard_notes
        }
        fn outgoing_sapling_notes(&self) -> &[OutgoingNoteSummary] {
            &self.outgoing_sapling_notes
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
                outgoing_orchard_notes,
                outgoing_sapling_notes,
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
    outgoing orchard notes: {}
    outgoing sapling notes: {}
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
                outgoing_orchard_notes,
                outgoing_sapling_notes,
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
                "outgoing_orchard_notes" => JsonValue::from(transaction.outgoing_orchard_notes),
                "outgoing_sapling_notes" => JsonValue::from(transaction.outgoing_sapling_notes),
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
        datetime: Option<u32>,
        status: Option<ConfirmationStatus>,
        blockheight: Option<BlockHeight>,
        kind: Option<TransactionKind>,
        value: Option<u64>,
        fee: Option<Option<u64>>,
        zec_price: Option<Option<f64>>,
        orchard_notes: Option<Vec<BasicNoteSummary>>,
        sapling_notes: Option<Vec<BasicNoteSummary>>,
        transparent_coins: Option<Vec<BasicCoinSummary>>,
        outgoing_orchard_notes: Option<Vec<OutgoingNoteSummary>>,
        outgoing_sapling_notes: Option<Vec<OutgoingNoteSummary>>,
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
                outgoing_orchard_notes: None,
                outgoing_sapling_notes: None,
            }
        }

        build_method!(txid, TxId);
        build_method!(datetime, u32);
        build_method!(status, ConfirmationStatus);
        build_method!(blockheight, BlockHeight);
        build_method!(kind, TransactionKind);
        build_method!(value, u64);
        build_method!(fee, Option<u64>);
        build_method!(zec_price, Option<f64>);
        build_method!(orchard_notes, Vec<BasicNoteSummary>);
        build_method!(sapling_notes, Vec<BasicNoteSummary>);
        build_method!(transparent_coins, Vec<BasicCoinSummary>);
        build_method!(outgoing_orchard_notes, Vec<OutgoingNoteSummary>);
        build_method!(outgoing_sapling_notes, Vec<OutgoingNoteSummary>);

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
                outgoing_orchard_notes: self.outgoing_orchard_notes.clone().ok_or(
                    BuildError::MissingField("orchard_outgoing_notes".to_string()),
                )?,
                outgoing_sapling_notes: self.outgoing_sapling_notes.clone().ok_or(
                    BuildError::MissingField("sapling_outgoing_notes".to_string()),
                )?,
            })
        }
    }

    impl Default for TransactionSummaryBuilder {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Basic note summary.
    ///
    /// Intended in the context of a transaction summary to provide the most useful data to user without cluttering up
    /// the interface. See [crate::wallet::summary::`NoteSummary`] for a note summary that is intended for use independently.
    ///
    /// A struct designed for conveniently displaying information to the user or converting to JSON to pass through an FFI.
    /// A "snapshot" of the state of the output in the wallet at the time the summary was constructed.
    /// Not to be used for internal logic in the system.
    #[derive(Clone, PartialEq, Debug)]
    pub struct BasicNoteSummary {
        value: u64,
        spend_status: SpendStatus,
        output_index: u32,
        memo: Option<String>,
        // TODO: add key id with address index, not implemented into sync engine yet
    }

    impl BasicNoteSummary {
        /// Creates an OrchardNoteSummary from parts
        pub fn from_parts(
            value: u64,
            spend_status: SpendStatus,
            output_index: u32,
            memo: Option<String>,
        ) -> Self {
            BasicNoteSummary {
                value,
                spend_status,
                output_index,
                memo,
            }
        }
        /// Gets value
        pub fn value(&self) -> u64 {
            self.value
        }

        /// Gets spend status
        pub fn spend_status(&self) -> SpendStatus {
            self.spend_status
        }

        /// Gets output index
        pub fn output_index(&self) -> u32 {
            self.output_index
        }
        /// Gets memo
        pub fn memo(&self) -> Option<&str> {
            self.memo.as_deref()
        }
    }

    impl std::fmt::Display for BasicNoteSummary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
                self.value, self.spend_status, self.output_index, memo,
            )
        }
    }

    impl From<BasicNoteSummary> for JsonValue {
        fn from(note: BasicNoteSummary) -> Self {
            json::object! {
                "value" => note.value,
                "spend_status" => note.spend_status.to_string(),
                "output_index" => note.output_index,
                "memo" => note.memo,
            }
        }
    }

    /// Wraps a vec of note summaries for the implementation of std::fmt::Display
    pub struct BasicNoteSummaries(Vec<BasicNoteSummary>);

    impl std::fmt::Display for BasicNoteSummaries {
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
    pub struct BasicCoinSummary {
        value: u64,
        spend_summary: SpendStatus,
        output_index: u32,
    }

    impl BasicCoinSummary {
        /// Creates a SaplingNoteSummary from parts
        pub fn from_parts(value: u64, spend_status: SpendStatus, output_index: u32) -> Self {
            BasicCoinSummary {
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
        pub fn spend_summary(&self) -> SpendStatus {
            self.spend_summary
        }

        /// Gets output index
        pub fn output_index(&self) -> u32 {
            self.output_index
        }
    }

    impl std::fmt::Display for BasicCoinSummary {
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
    impl From<BasicCoinSummary> for JsonValue {
        fn from(note: BasicCoinSummary) -> Self {
            json::object! {
                "value" => note.value,
                "spend_status" => note.spend_summary.to_string(),
                "output_index" => note.output_index,
            }
        }
    }

    /// Wraps a vec of transparent coin summaries for the implementation of std::fmt::Display
    pub struct BasicCoinSummaries(Vec<BasicCoinSummary>);

    impl std::fmt::Display for BasicCoinSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for coin in &self.0 {
                write!(f, "\n{}", coin)?;
            }
            Ok(())
        }
    }

    /// Outgoing note summary.
    /// A struct designed for conveniently displaying information to the user or converting to JSON to pass through an FFI.
    /// A "snapshot" of the state of the outgoing note in the wallet at the time the summary was constructed.
    /// Not to be used for internal logic in the system.
    #[derive(Clone, PartialEq, Debug)]
    pub struct OutgoingNoteSummary {
        pub output_index: u16,
        pub key_id: KeyId,
        pub value: u64,
        pub memo: Option<String>,
        pub recipient: String,
        pub recipient_unified_address: Option<String>,
    }

    impl std::fmt::Display for OutgoingNoteSummary {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let memo = self.memo.clone().unwrap_or_default();
            let recipient_unified_address = self
                .recipient_unified_address
                .clone()
                .unwrap_or_else(|| "not available".to_string());

            write!(
                f,
                "\t{{
            output index: {}
            account id: {}
            value: {}
            memo: {}
            recipient: {}
            recipient unified address: {}
        }}",
                self.output_index,
                u32::from(self.key_id.account_id),
                self.value,
                memo,
                self.recipient,
                recipient_unified_address,
            )
        }
    }

    impl From<OutgoingNoteSummary> for JsonValue {
        fn from(note: OutgoingNoteSummary) -> Self {
            json::object! {
                "account_id" => u32::from(note.key_id.account_id),
                "output_index" => note.output_index,
                "value" => note.value,
                "memo" => note.memo,
                "recipient" => note.recipient,
                "recipient_unified_address" => note.recipient_unified_address
            }
        }
    }

    /// Wraps a vec of orchard note summaries for the implementation of std::fmt::Display
    pub struct OutgoingNoteSummaries(Vec<OutgoingNoteSummary>);

    impl std::fmt::Display for OutgoingNoteSummaries {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            for note in &self.0 {
                write!(f, "\n{}", note)?;
            }
            Ok(())
        }
    }
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

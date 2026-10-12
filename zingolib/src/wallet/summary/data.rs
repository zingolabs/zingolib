//! Data structures for wallet summaries.

use chrono::DateTime;
use json::JsonValue;

use zcash_protocol::memo::Memo;
use zcash_protocol::{PoolType, TxId, consensus::BlockHeight};

use pepper_sync::keys::transparent::TransparentScope;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::wallet::output::SpendStatus;

/// Scope enum with `std::fmt::Display` impl for use with summaries.
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    External,
    Internal,
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Scope::External => "external",
                Scope::Internal => "internal",
            }
        )
    }
}

impl From<zip32::Scope> for Scope {
    fn from(value: zip32::Scope) -> Self {
        match value {
            zip32::Scope::External => Scope::External,
            zip32::Scope::Internal => Scope::Internal,
        }
    }
}

/// Transaction kind.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TransactionKind {
    /// Sent transaction.
    Sent(SendType),
    /// Received transaction.
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

/// Send type.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SendType {
    /// Transaction is sending funds to recipient other than the creator.
    Send,
    /// Transaction is only sending funds from transparent pool to the creator's shielded pool.
    Shield,
    /// Transaction is only sending funds to the creator's address(es) and is not a shield.
    SendToSelf,
}

/// Names of the given pools, e.g. `["Orchard", "Ironwood"]`.
fn pool_names(pools: &[PoolType]) -> Vec<String> {
    pools.iter().map(ToString::to_string).collect()
}

/// Formats a list of pools for display, e.g. "Orchard, Ironwood".
fn display_pools(pools: &[PoolType]) -> String {
    pool_names(pools).join(", ")
}

/// Converts a list of pools to a JSON array of pool names.
fn pools_to_json(pools: &[PoolType]) -> JsonValue {
    JsonValue::from(pool_names(pools))
}

/// The rendering the pre-typed summaries carried: a text memo renders as
/// its string; empty and non-text memos render as the absent memo did.
/// Display and JSON bytes stay identical to the era when construction
/// dropped non-text memos; the typed field carries them losslessly.
fn text_memo(memo: &Memo) -> Option<String> {
    if let Memo::Text(text) = memo {
        Some(text.to_string())
    } else {
        None
    }
}

/// The pools flagged present, in protocol order. `present` indices are
/// (transparent, sapling, orchard, ironwood). This function is the single
/// definition of that order for pool lists exposed by summaries.
pub fn pools_present(present: [bool; 4]) -> Vec<PoolType> {
    let [transparent, sapling, orchard, ironwood] = present;
    [
        (transparent, PoolType::TRANSPARENT),
        (sapling, PoolType::SAPLING),
        (orchard, PoolType::ORCHARD),
        (ironwood, PoolType::IRONWOOD),
    ]
    .into_iter()
    .filter_map(|(present, pool)| present.then_some(pool))
    .collect()
}

/// Transaction summary.
#[derive(Clone, PartialEq, Debug)]
pub struct TransactionSummary {
    pub txid: TxId,
    pub datetime: u32,
    pub status: ConfirmationStatus,
    pub blockheight: BlockHeight,
    pub kind: TransactionKind,
    pub value: u64,
    pub fee: Option<u64>,
    pub zec_price: Option<f32>,
    /// Pools of this wallet's outputs spent to fund this transaction.
    /// Empty for received transactions.
    pub pools_sent_from: Vec<PoolType>,
    pub ironwood_notes: Vec<BasicNoteSummary>,
    pub orchard_notes: Vec<BasicNoteSummary>,
    pub sapling_notes: Vec<BasicNoteSummary>,
    pub transparent_coins: Vec<BasicCoinSummary>,
    pub outgoing_ironwood_notes: Vec<OutgoingNoteSummary>,
    pub outgoing_orchard_notes: Vec<OutgoingNoteSummary>,
    pub outgoing_sapling_notes: Vec<OutgoingNoteSummary>,
    pub outgoing_transparent_coins: Vec<OutgoingCoinSummary>,
}

impl TransactionSummary {
    /// Pools this transaction's wallet-received outputs arrived into, in protocol order
    /// (transparent, sapling, orchard, ironwood).
    #[must_use]
    pub fn pools_received(&self) -> Vec<PoolType> {
        pools_present([
            !self.transparent_coins.is_empty(),
            !self.sapling_notes.is_empty(),
            !self.orchard_notes.is_empty(),
            !self.ironwood_notes.is_empty(),
        ])
    }

    /// Whether this send-to-self is an Orchard -> Ironwood migration: the wallet
    /// spent Orchard notes and received the value into its Ironwood pool.
    ///
    /// Detected purely from pool movement (spent from Orchard, received into
    /// Ironwood), so it also covers an Orchard -> Ironwood self-transfer made
    /// outside the migration machinery. The receiving side depends on the
    /// Ironwood note being recorded on this transaction, so this only reports
    /// `true` once that note is scanned into [`Self::ironwood_notes`].
    #[must_use]
    pub fn is_orchard_to_ironwood_migration(&self) -> bool {
        self.pools_sent_from.contains(&PoolType::ORCHARD)
            && self.pools_received().contains(&PoolType::IRONWOOD)
    }

    /// The shielded note summaries paired with their pool, newest pool first
    /// (ironwood, orchard, sapling), the order value transfers are listed in.
    pub fn shielded_notes_by_pool(&self) -> [(&[BasicNoteSummary], PoolType); 3] {
        [
            (self.ironwood_notes.as_slice(), PoolType::IRONWOOD),
            (self.orchard_notes.as_slice(), PoolType::ORCHARD),
            (self.sapling_notes.as_slice(), PoolType::SAPLING),
        ]
    }

    /// The sum of every output this transaction delivered to the wallet's own
    /// addresses: all wallet-received shielded notes plus transparent coins.
    pub fn self_received_value(&self) -> u64 {
        self.shielded_notes_by_pool()
            .into_iter()
            .flat_map(|(notes, _)| notes.iter().map(|note| note.value))
            .chain(self.transparent_coins.iter().map(|coin| coin.value))
            .sum()
    }

    #[must_use]
    pub fn balance_delta(&self) -> Option<i64> {
        match self.kind {
            TransactionKind::Sent(SendType::Send) => {
                self.fee.map(|fee| -((self.value + fee) as i64))
            }
            TransactionKind::Sent(SendType::Shield | SendType::SendToSelf) => {
                self.fee.map(|fee| -(fee as i64))
            }
            TransactionKind::Received => Some(self.value as i64),
        }
    }
    /// Prepares the fields in the summary for display
    #[must_use]
    pub fn prepare_for_display(
        &self,
    ) -> (
        String,
        String,
        String,
        BasicNoteSummaries,
        BasicNoteSummaries,
        BasicNoteSummaries,
        BasicCoinSummaries,
        OutgoingNoteSummaries,
        OutgoingNoteSummaries,
        OutgoingNoteSummaries,
        OutgoingCoinSummaries,
    ) {
        let datetime = if let Some(dt) = DateTime::from_timestamp(i64::from(self.datetime), 0) {
            format!("{dt}")
        } else {
            "not available".to_string()
        };
        let fee = if let Some(f) = self.fee {
            f.to_string()
        } else {
            "not available".to_string()
        };
        let zec_price = if let Some(price) = self.zec_price {
            price.to_string()
        } else {
            "not available".to_string()
        };
        let ironwood_notes = BasicNoteSummaries(self.ironwood_notes.clone());
        let orchard_notes = BasicNoteSummaries(self.orchard_notes.clone());
        let sapling_notes = BasicNoteSummaries(self.sapling_notes.clone());
        let transparent_coins = BasicCoinSummaries(self.transparent_coins.clone());
        let outgoing_ironwood_notes = OutgoingNoteSummaries(self.outgoing_ironwood_notes.clone());
        let outgoing_orchard_notes = OutgoingNoteSummaries(self.outgoing_orchard_notes.clone());
        let outgoing_sapling_notes = OutgoingNoteSummaries(self.outgoing_sapling_notes.clone());
        let outgoing_transparent_coins =
            OutgoingCoinSummaries(self.outgoing_transparent_coins.clone());

        (
            datetime,
            fee,
            zec_price,
            ironwood_notes,
            orchard_notes,
            sapling_notes,
            transparent_coins,
            outgoing_ironwood_notes,
            outgoing_orchard_notes,
            outgoing_sapling_notes,
            outgoing_transparent_coins,
        )
    }
}

impl std::fmt::Display for TransactionSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (
            datetime,
            fee,
            zec_price,
            ironwood_notes,
            orchard_notes,
            sapling_notes,
            transparent_coins,
            outgoing_ironwood_notes,
            outgoing_orchard_notes,
            outgoing_sapling_notes,
            outgoing_transparent_coins,
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
    pools sent from: {}
    ironwood notes: {}
    orchard notes: {}
    sapling notes: {}
    transparent coins: {}
    outgoing ironwood notes: {}
    outgoing orchard notes: {}
    outgoing sapling notes: {}
    outgoing transparent coins: {}
}}",
            self.txid,
            datetime,
            self.status,
            u64::from(self.blockheight),
            self.kind,
            self.value,
            fee,
            zec_price,
            display_pools(&self.pools_sent_from),
            ironwood_notes,
            orchard_notes,
            sapling_notes,
            transparent_coins,
            outgoing_ironwood_notes,
            outgoing_orchard_notes,
            outgoing_sapling_notes,
            outgoing_transparent_coins,
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
            "pools_sent_from" => pools_to_json(&transaction.pools_sent_from),
            "ironwood_notes" => JsonValue::from(transaction.ironwood_notes),
            "orchard_notes" => JsonValue::from(transaction.orchard_notes),
            "sapling_notes" => JsonValue::from(transaction.sapling_notes),
            "transparent_coins" => JsonValue::from(transaction.transparent_coins),
            "outgoing_ironwood_notes" => JsonValue::from(transaction.outgoing_ironwood_notes),
            "outgoing_orchard_notes" => JsonValue::from(transaction.outgoing_orchard_notes),
            "outgoing_sapling_notes" => JsonValue::from(transaction.outgoing_sapling_notes),
            "outgoing_transparent_coins" => JsonValue::from(transaction.outgoing_transparent_coins),
        }
    }
}

/// Wraps a vec of transaction summaries for the implementation of `std::fmt::Display`
#[derive(PartialEq, Debug)]
pub struct TransactionSummaries(pub Vec<TransactionSummary>);

impl TransactionSummaries {
    /// Creates a new `TransactionSummaries` struct
    #[must_use]
    pub fn new(transaction_summaries: Vec<TransactionSummary>) -> Self {
        TransactionSummaries(transaction_summaries)
    }
    /// Implicitly dispatch to the wrapped data
    pub fn iter(&self) -> std::slice::Iter<'_, TransactionSummary> {
        self.0.iter()
    }
    /// Sum total of all fees paid in sending transactions
    #[must_use]
    pub fn paid_fees(&self) -> u64 {
        self.iter()
            .filter_map(|summary| {
                if matches!(summary.kind, TransactionKind::Sent(_)) && summary.status.is_confirmed()
                {
                    summary.fee
                } else {
                    None
                }
            })
            .sum()
    }
    /// A Vec of the txids
    #[must_use]
    pub fn txids(&self) -> Vec<TxId> {
        self.iter().map(|summary| summary.txid).collect()
    }
}

impl std::fmt::Display for TransactionSummaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for transaction_summary in &self.0 {
            write!(f, "\n{transaction_summary}")?;
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

/// Note summary.
///
/// Intended for returning a standalone summary of all notes to the user / consumer outside the context of transactions.
#[allow(missing_docs)]
#[derive(Debug)]
pub struct NoteSummary {
    pub value: u64,
    pub status: ConfirmationStatus,
    pub block_height: BlockHeight,
    pub spend_status: SpendStatus,
    pub memo: Memo,
    pub time: u32,
    pub txid: TxId,
    pub output_index: u32,
    pub account_id: zip32::AccountId,
    pub scope: Scope,
}

impl std::fmt::Display for NoteSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let memo = text_memo(&self.memo).unwrap_or_default();
        let time = if let Some(dt) = chrono::DateTime::from_timestamp(i64::from(self.time), 0) {
            format!("{dt}")
        } else {
            "not available".to_string()
        };

        write!(
            f,
            "{{
                value: {}
                status: {} at block height {}
                spend status: {}
                memo: {}
                time: {}
                txid: {}
                output index: {}
                account id: {}
                scope: {}
            }}",
            self.value,
            self.status,
            self.block_height,
            self.spend_status,
            memo,
            time,
            self.txid,
            self.output_index,
            u32::from(self.account_id),
            self.scope
        )
    }
}

impl From<NoteSummary> for json::JsonValue {
    fn from(note: NoteSummary) -> Self {
        json::object! {
            "value" => note.value,
            "status" => format!("{} at block height {}", note.status, note.block_height),
            "spend_status" => note.spend_status.to_string(),
            "memo" => text_memo(&note.memo),
            "time" => note.time,
            "txid" => note.txid.to_string(),
            "output_index" => note.output_index,
            "account_id" => u32::from(note.account_id),
            "scope" => note.scope.to_string(),
        }
    }
}

/// A wrapper struct for implementing display and json on a vec of note summaries
#[derive(Debug)]
pub struct NoteSummaries(Vec<NoteSummary>);

impl NoteSummaries {
    /// Creates a new `NoteSummaries`
    #[must_use]
    pub fn new(note_summaries: Vec<NoteSummary>) -> Self {
        NoteSummaries(note_summaries)
    }
}

impl<'a> std::iter::IntoIterator for &'a NoteSummaries {
    type Item = &'a NoteSummary;
    type IntoIter = std::slice::Iter<'a, NoteSummary>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl std::ops::Deref for NoteSummaries {
    type Target = Vec<NoteSummary>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for NoteSummaries {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl std::ops::Index<usize> for NoteSummaries {
    type Output = NoteSummary; // The type of the value returned by the index

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index] // Forward the indexing operation to the underlying data structure
    }
}

impl std::fmt::Display for NoteSummaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for value_transfer in &self.0 {
            write!(f, "\n{value_transfer}")?;
        }
        Ok(())
    }
}

impl From<NoteSummaries> for json::JsonValue {
    fn from(note_summaries: NoteSummaries) -> Self {
        let note_summaries: Vec<json::JsonValue> = note_summaries
            .0
            .into_iter()
            .map(json::JsonValue::from)
            .collect();
        json::object! {
            "note_summaries" => note_summaries

        }
    }
}

/// Basic note summary.
///
/// Intended in the context of a transaction summary to provide the most useful data to user without cluttering up
/// the interface. See [`crate::wallet::summary::``NoteSummary`] for a note summary that is intended for use independently.
#[derive(Clone, PartialEq, Debug)]
pub struct BasicNoteSummary {
    pub value: u64,
    pub spend_status: SpendStatus,
    pub output_index: u32,
    pub memo: Memo,
    // TODO: add key id with address index, not implemented into sync engine yet
}

impl BasicNoteSummary {
    /// Creates a `BasicNoteSummary` from parts
    #[must_use]
    pub fn from_parts(
        value: u64,
        spend_status: SpendStatus,
        output_index: u32,
        memo: Memo,
    ) -> Self {
        BasicNoteSummary {
            value,
            spend_status,
            output_index,
            memo,
        }
    }
}

impl std::fmt::Display for BasicNoteSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let memo = text_memo(&self.memo).unwrap_or_default();
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
            "memo" => text_memo(&note.memo),
        }
    }
}

/// Wraps a vec of note summaries for the implementation of `std::fmt::Display`
pub struct BasicNoteSummaries(Vec<BasicNoteSummary>);

impl std::fmt::Display for BasicNoteSummaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for note in &self.0 {
            write!(f, "\n{note}")?;
        }
        Ok(())
    }
}

/// Coin summary.
///
/// Intended for returning a standalone summary of all transparent coins to the user / consumer outside the context of
/// transactions.
#[allow(missing_docs)]
pub struct CoinSummary {
    pub value: u64,
    pub status: ConfirmationStatus,
    pub block_height: BlockHeight,
    pub spend_status: SpendStatus,
    pub time: u32,
    pub txid: TxId,
    pub output_index: u32,
    pub account_id: zip32::AccountId,
    pub scope: TransparentScope,
    pub address_index: u32,
}

impl std::fmt::Display for CoinSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let time = if let Some(dt) = chrono::DateTime::from_timestamp(i64::from(self.time), 0) {
            format!("{dt}")
        } else {
            "not available".to_string()
        };

        write!(
            f,
            "{{
                value: {}
                status: {} at block height {}
                spend status: {}
                time: {}
                txid: {}
                output index: {}
                account id: {}
                scope: {}
                address_index: {}
            }}",
            self.value,
            self.status,
            self.block_height,
            self.spend_status,
            time,
            self.txid,
            self.output_index,
            u32::from(self.account_id),
            self.scope,
            self.address_index,
        )
    }
}

impl From<CoinSummary> for json::JsonValue {
    fn from(coin: CoinSummary) -> Self {
        json::object! {
            "value" => coin.value,
            "status" => format!("{} at block height {}", coin.status, coin.block_height),
            "spend_status" => coin.spend_status.to_string(),
            "time" => coin.time,
            "txid" => coin.txid.to_string(),
            "output_index" => coin.output_index,
            "account_id" => u32::from(coin.account_id),
            "scope" => coin.scope.to_string(),
            "address_index" => coin.address_index
        }
    }
}

/// Transparent coin summary.
// TODO: add scope to distinguish "refund" scope value transfers
#[derive(Clone, PartialEq, Debug)]
pub struct BasicCoinSummary {
    pub value: u64,
    pub spend_summary: SpendStatus,
    pub output_index: u32,
}

impl BasicCoinSummary {
    /// Creates a `BasicCoinSummary` from parts
    #[must_use]
    pub fn from_parts(value: u64, spend_status: SpendStatus, output_index: u32) -> Self {
        BasicCoinSummary {
            value,
            spend_summary: spend_status,
            output_index,
        }
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

/// Wraps a vec of transparent coin summaries for the implementation of `std::fmt::Display`
pub struct BasicCoinSummaries(Vec<BasicCoinSummary>);

impl std::fmt::Display for BasicCoinSummaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for coin in &self.0 {
            write!(f, "\n{coin}")?;
        }
        Ok(())
    }
}

/// Outgoing note summary.
#[derive(Clone, PartialEq, Debug)]
pub struct OutgoingNoteSummary {
    pub value: u64,
    pub memo: Memo,
    pub recipient: String,
    pub recipient_unified_address: Option<String>,
    pub output_index: u32,
    pub account_id: zip32::AccountId,
    pub scope: Scope,
}

impl std::fmt::Display for OutgoingNoteSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let memo = text_memo(&self.memo).unwrap_or_default();
        let recipient_unified_address = self
            .recipient_unified_address
            .clone()
            .unwrap_or_else(|| "not available".to_string());

        write!(
            f,
            "\t{{
            value: {}
            memo: {}
            recipient: {}
            recipient unified address: {}
            output index: {}
            account id: {}
            scope: {}
        }}",
            self.value,
            memo,
            self.recipient,
            recipient_unified_address,
            self.output_index,
            u32::from(self.account_id),
            self.scope,
        )
    }
}

impl From<OutgoingNoteSummary> for JsonValue {
    fn from(note: OutgoingNoteSummary) -> Self {
        json::object! {
            "value" => note.value,
            "memo" => text_memo(&note.memo),
            "recipient" => note.recipient,
            "recipient_unified_address" => note.recipient_unified_address,
            "output_index" => note.output_index,
            "account_id" => u32::from(note.account_id),
            "scope" => note.scope.to_string(),
        }
    }
}

/// Wraps a vec of orchard note summaries for the implementation of `std::fmt::Display`
pub struct OutgoingNoteSummaries(Vec<OutgoingNoteSummary>);

impl std::fmt::Display for OutgoingNoteSummaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for note in &self.0 {
            write!(f, "\n{note}")?;
        }
        Ok(())
    }
}

/// Outgoing coin summary.
#[derive(Clone, PartialEq, Debug)]
pub struct OutgoingCoinSummary {
    pub value: u64,
    pub recipient: String,
    pub output_index: u32,
}

impl std::fmt::Display for OutgoingCoinSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\t{{
            value: {}
            recipient: {}
            output index: {}
        }}",
            self.value, self.recipient, self.output_index,
        )
    }
}

impl From<OutgoingCoinSummary> for JsonValue {
    fn from(note: OutgoingCoinSummary) -> Self {
        json::object! {
            "value" => note.value,
            "recipient" => note.recipient,
            "output_index" => note.output_index,
        }
    }
}

/// Wraps a vec of orchard note summaries for the implementation of `std::fmt::Display`
pub struct OutgoingCoinSummaries(Vec<OutgoingCoinSummary>);

impl std::fmt::Display for OutgoingCoinSummaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for coin in &self.0 {
            write!(f, "\n{coin}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BasicNoteSummary, SendType, TransactionKind, TransactionSummary};
    use crate::wallet::output::SpendStatus;
    use zcash_protocol::{PoolType, TxId};
    use zingo_status::confirmation_status::ConfirmationStatus;

    fn note(value: u64) -> BasicNoteSummary {
        BasicNoteSummary::from_parts(value, SpendStatus::Unspent, 0, super::Memo::Empty)
    }

    /// A minimal send-to-self summary with the given funding pools and received
    /// Ironwood/Orchard notes. Every other field is empty or a neutral default.
    fn self_send_summary(
        pools_sent_from: Vec<PoolType>,
        ironwood_notes: Vec<BasicNoteSummary>,
        orchard_notes: Vec<BasicNoteSummary>,
    ) -> TransactionSummary {
        TransactionSummary {
            txid: TxId::from_bytes([0; 32]),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(10u32.into()),
            blockheight: 10u32.into(),
            kind: TransactionKind::Sent(SendType::SendToSelf),
            value: 0,
            fee: Some(0),
            zec_price: None,
            pools_sent_from,
            ironwood_notes,
            orchard_notes,
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_ironwood_notes: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        }
    }

    #[test]
    fn orchard_funded_ironwood_receive_is_a_migration() {
        // The shape of a migration part: Orchard notes spent, value landing in
        // the wallet's own Ironwood pool.
        let summary = self_send_summary(vec![PoolType::ORCHARD], vec![note(100_000)], vec![]);
        assert!(summary.is_orchard_to_ironwood_migration());
    }

    #[test]
    fn orchard_to_orchard_note_split_is_not_a_migration() {
        // A note-splitting round funds from Orchard and receives Orchard change,
        // with nothing landing in the Ironwood pool.
        let summary = self_send_summary(vec![PoolType::ORCHARD], vec![], vec![note(100_000)]);
        assert!(!summary.is_orchard_to_ironwood_migration());
    }

    #[test]
    fn ironwood_receive_without_orchard_funding_is_not_a_migration() {
        // Value arriving in Ironwood but not funded from Orchard is not a
        // migration, whether the funding side is empty or another pool.
        let unfunded = self_send_summary(vec![], vec![note(100_000)], vec![]);
        assert!(!unfunded.is_orchard_to_ironwood_migration());

        let ironwood_funded =
            self_send_summary(vec![PoolType::IRONWOOD], vec![note(100_000)], vec![]);
        assert!(!ironwood_funded.is_orchard_to_ironwood_migration());
    }
}

//! Types and impls for displaying wallet transaction data to the user / consumer.

use pepper_sync::{
    keys::transparent::TransparentScope,
    wallet::{NoteInterface, OutputInterface as _, TransparentCoin},
};
use zcash_primitives::{consensus::BlockHeight, memo::Memo, transaction::TxId};
use zingo_status::confirmation_status::ConfirmationStatus;

use super::{output::SpendStatus, LightWallet};

// TODO: move data::summaries and value transfer / transaction summary methods here

/// Scope enum with std::fmt::Display impl for use with summaries.
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
    pub memo: Option<String>,
    pub time: u32,
    pub txid: TxId,
    pub output_index: u16,
    pub account_id: zip32::AccountId,
    pub scope: Scope,
}

impl std::fmt::Display for NoteSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let memo = self.memo.clone().unwrap_or_default();
        let time = if let Some(dt) = chrono::DateTime::from_timestamp(self.time as i64, 0) {
            format!("{}", dt)
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
            "memo" => note.memo,
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
    /// Creates a new NoteSummaries
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
            write!(f, "\n{}", value_transfer)?;
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
    pub output_index: u16,
    pub account_id: zip32::AccountId,
    pub scope: TransparentScope,
    pub address_index: u32,
}

impl std::fmt::Display for CoinSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let time = if let Some(dt) = chrono::DateTime::from_timestamp(self.time as i64, 0) {
            format!("{}", dt)
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

impl LightWallet {
    pub fn note_summaries<N>(&self, include_spent_notes: bool) -> NoteSummaries
    where
        N: NoteInterface<KeyId = pepper_sync::keys::KeyId>,
    {
        let note_summaries = self
            .wallet_outputs::<N>()
            .into_iter()
            .filter(|&note| {
                if include_spent_notes {
                    true
                } else {
                    note.spending_transaction().is_none()
                }
            })
            .map(|note| {
                let memo = if let Memo::Text(memo_text) = note.memo() {
                    Some(memo_text.to_string())
                } else {
                    None
                };
                let transaction = self.output_transaction(note);

                NoteSummary {
                    value: note.value(),
                    status: transaction.status(),
                    block_height: transaction.status().get_height(),
                    spend_status: self.output_spend_status(note),
                    memo,
                    time: transaction.datetime(),
                    txid: note.output_id().txid(),
                    output_index: note.output_id().output_index(),
                    account_id: note.key_id().account_id,
                    scope: note.key_id().scope.into(),
                }
            })
            .collect();

        NoteSummaries::new(note_summaries)
    }

    pub fn coin_summaries(&self, include_spent_coins: bool) -> Vec<CoinSummary> {
        self.wallet_outputs::<TransparentCoin>()
            .into_iter()
            .filter(|&coin| {
                if include_spent_coins {
                    true
                } else {
                    coin.spending_transaction().is_none()
                }
            })
            .map(|coin| {
                let transaction = self.output_transaction(coin);

                CoinSummary {
                    value: coin.value(),
                    status: transaction.status(),
                    block_height: transaction.status().get_height(),
                    spend_status: self.output_spend_status(coin),
                    time: transaction.datetime(),
                    txid: coin.output_id().txid(),
                    output_index: coin.output_id().output_index(),
                    account_id: coin.key_id().account_id(),
                    scope: coin.key_id().scope(),
                    address_index: coin.key_id().address_index(),
                }
            })
            .collect()
    }
}

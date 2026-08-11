//! The editorial layer: the house perspective over the canonical wallet
//! record, deriving presentation-facing statements that never feed wallet
//! logic.

pub mod finsight;
pub mod value_transfer;

use std::collections::{HashMap, HashSet};

use zcash_protocol::PoolType;

use crate::lightclient::LightClient;
use crate::wallet::LightWallet;
use crate::wallet::error::{KeyError, SummaryError};
use crate::wallet::summary::data::{
    BasicNoteSummary, OutgoingCoinSummary, OutgoingNoteSummary, Scope, SendType, TransactionKind,
    TransactionSummary, pools_present,
};

use finsight::{
    Finsight, TotalMemoBytesToAddress, TotalSendsToAddress, TotalValueToAddress,
    ValuesSentToAddress,
};
use value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransfer, ValueTransferKind, ValueTransfers,
};

/// The summary accessors that serve only the editorial derivation.
impl TransactionSummary {
    /// The shielded note summaries paired with their pool, newest pool first
    /// (ironwood, orchard, sapling), the order value transfers are listed in.
    pub(crate) fn shielded_notes_by_pool(&self) -> [(&[BasicNoteSummary], PoolType); 3] {
        [
            (self.ironwood_notes.as_slice(), PoolType::IRONWOOD),
            (self.orchard_notes.as_slice(), PoolType::ORCHARD),
            (self.sapling_notes.as_slice(), PoolType::SAPLING),
        ]
    }

    /// All memos on this transaction's wallet-received shielded notes, in pool
    /// order ironwood, orchard, sapling.
    pub(crate) fn received_memos(&self) -> Vec<String> {
        self.shielded_notes_by_pool()
            .into_iter()
            .flat_map(|(notes, _)| notes.iter().filter_map(|note| note.memo.clone()))
            .collect()
    }

    /// The sum of every output this transaction delivered to the wallet's own
    /// addresses: all wallet-received shielded notes plus transparent coins.
    pub(crate) fn self_received_value(&self) -> u64 {
        self.shielded_notes_by_pool()
            .into_iter()
            .flat_map(|(notes, _)| notes.iter().map(|note| note.value))
            .chain(self.transparent_coins.iter().map(|coin| coin.value))
            .sum()
    }

    /// The sum of the wallet-received shielded notes that carry a memo,
    /// excluding memo-less change.
    pub(crate) fn received_memo_value(&self) -> u64 {
        self.shielded_notes_by_pool()
            .into_iter()
            .flat_map(|(notes, _)| notes.iter())
            .filter(|note| note.memo.is_some())
            .map(|note| note.value)
            .sum()
    }
}

/// Creates one value transfer of `kind` for each shielded pool the transaction
/// received notes into, newest pool first (ironwood, orchard, sapling).
fn shielded_pool_value_transfers(
    transaction: &TransactionSummary,
    kind: ValueTransferKind,
) -> Vec<ValueTransfer> {
    transaction
        .shielded_notes_by_pool()
        .into_iter()
        .filter(|(notes, _)| !notes.is_empty())
        .map(|(notes, pool)| {
            ValueTransfer::from_summary(
                transaction,
                kind,
                notes.iter().map(|output| output.value).sum(),
                None,
                vec![pool],
                notes.iter().filter_map(|note| note.memo.clone()).collect(),
            )
        })
        .collect()
}

fn self_send_value_transfer(
    transaction: &TransactionSummary,
    kind: SelfSendValueTransfer,
    value: u64,
    memos: Vec<String>,
) -> ValueTransfer {
    ValueTransfer::from_summary(
        transaction,
        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(kind)),
        value,
        None,
        transaction.pools_received(),
        memos,
    )
}

/// Pairs each note with `pool`, for chaining outgoing notes across pools.
fn tag_pool(
    notes: &[OutgoingNoteSummary],
    pool: PoolType,
) -> impl Iterator<Item = (&OutgoingNoteSummary, PoolType)> {
    notes.iter().map(move |note| (note, pool))
}

/// Creates value transfers for all notes in a transaction that are sent to another
/// recipient.  A value transfer is a group of all notes to a specific receiver in a transaction.
/// The value transfer list is sorted by the output index of the notes.
fn create_send_value_transfers(
    wallet: &LightWallet,
    transaction: &TransactionSummary,
) -> Result<Vec<ValueTransfer>, KeyError> {
    let mut value_transfers: Vec<ValueTransfer> = Vec::new();
    let outgoing_notes = tag_pool(&transaction.outgoing_ironwood_notes, PoolType::IRONWOOD)
        .chain(tag_pool(
            &transaction.outgoing_orchard_notes,
            PoolType::ORCHARD,
        ))
        .chain(tag_pool(
            &transaction.outgoing_sapling_notes,
            PoolType::SAPLING,
        ))
        .collect::<Vec<_>>();
    let outgoing_coins = &transaction.outgoing_transparent_coins;
    let mut addresses = HashSet::new();

    outgoing_notes.iter().try_for_each(|&(note, _)| {
        if note.scope == Scope::External && wallet.is_wallet_address(&note.recipient)?.is_none() {
            let encoded_address = note
                .recipient_unified_address
                .clone()
                .unwrap_or(note.recipient.clone());
            addresses.insert(encoded_address);
        }

        Ok::<(), KeyError>(())
    })?;
    outgoing_coins.iter().try_for_each(|coin| {
        if wallet.is_wallet_address(&coin.recipient)?.is_none() {
            addresses.insert(coin.recipient.clone());
        }

        Ok::<(), KeyError>(())
    })?;
    let mut addresses = addresses.into_iter().collect::<Vec<_>>();
    addresses.sort();
    for address in addresses {
        let outgoing_notes_to_address: Vec<(&OutgoingNoteSummary, PoolType)> = outgoing_notes
            .iter()
            .filter(|&&(note, _)| {
                let query_address = if let Some(ua) = note.recipient_unified_address.clone() {
                    ua
                } else {
                    note.recipient.clone()
                };
                query_address == address
            })
            .copied()
            .collect();
        let outgoing_coins_to_address: Vec<&OutgoingCoinSummary> = outgoing_coins
            .iter()
            .filter(|&coin| coin.recipient.clone() == address)
            .collect();
        let value: u64 = outgoing_notes_to_address
            .iter()
            .map(|&(note, _)| note.value)
            .chain(outgoing_coins_to_address.iter().map(|&coin| coin.value))
            .sum();
        let memos: Vec<String> = outgoing_notes_to_address
            .iter()
            .filter_map(|&(note, _)| note.memo.clone())
            .collect();
        let has_notes_in = |pool: PoolType| {
            outgoing_notes_to_address
                .iter()
                .any(|&(_, note_pool)| note_pool == pool)
        };
        let pools_received = pools_present([
            !outgoing_coins_to_address.is_empty(),
            has_notes_in(PoolType::SAPLING),
            has_notes_in(PoolType::ORCHARD),
            has_notes_in(PoolType::IRONWOOD),
        ]);
        value_transfers.push(ValueTransfer::from_summary(
            transaction,
            ValueTransferKind::Sent(SentValueTransfer::Send),
            value,
            Some(address),
            pools_received,
            memos,
        ));
    }

    Ok(value_transfers)
}

fn finsight_from(value_transfers: &ValueTransfers) -> Finsight {
    let mut values_by_address: HashMap<String, Vec<u64>> = HashMap::new();
    let mut memobytes_by_address: HashMap<String, usize> = HashMap::new();
    for value_transfer in value_transfers.iter() {
        if let ValueTransferKind::Sent(SentValueTransfer::Send) = value_transfer.kind {
            let address = value_transfer
                .recipient_address
                .clone()
                .expect("sent value transfer should always have a recipient_address");
            values_by_address
                .entry(address.clone())
                .or_default()
                .push(value_transfer.value);
            *memobytes_by_address.entry(address).or_default() +=
                value_transfer.memos.iter().map(String::len).sum::<usize>();
        }
    }

    Finsight {
        values_sent_to_address: ValuesSentToAddress(values_by_address),
        total_memobytes_to_address: TotalMemoBytesToAddress(memobytes_by_address),
    }
}

/// The editorial reductions over the wallet.
impl LightWallet {
    /// Provides a list of value transfers related to this capability.
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    pub async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError> {
        let mut value_transfers: Vec<ValueTransfer> = Vec::new();
        let transaction_summaries = self.transaction_summaries(sort_highest_to_lowest).await?.0;

        for transaction in transaction_summaries {
            match transaction.kind {
                TransactionKind::Sent(SendType::Send) => {
                    // create 1 sent value transfer for each non-self recipient address
                    // if recipient_ua is available it overrides recipient_address
                    value_transfers.append(&mut create_send_value_transfers(self, &transaction)?);

                    // create 1 memo-to-self if any number of memos are received in the sending transaction
                    let memos = transaction.received_memos();
                    if !memos.is_empty() {
                        value_transfers.push(self_send_value_transfer(
                            &transaction,
                            SelfSendValueTransfer::MemoToSelf,
                            transaction.received_memo_value(),
                            memos,
                        ));
                    }
                }
                TransactionKind::Sent(SendType::Shield) => {
                    // create 1 shielding value transfer for each pool shielded to
                    value_transfers.extend(shielded_pool_value_transfers(
                        &transaction,
                        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                            SelfSendValueTransfer::Shield,
                        )),
                    ));
                }
                TransactionKind::Sent(SendType::SendToSelf) => {
                    // create 1 migration transfer if this moved Orchard funds into the Ironwood
                    // pool (migration parts self-receive an empty Ironwood memo, so this must win
                    // over the memo check), else 1 memo-to-self if a sending transaction receives
                    // any number of memos, otherwise 1 basic send-to-self so every transaction
                    // creates at least 1 value transfer.
                    // (deshield and other pool-movement kinds may join this list later.)
                    let memos = transaction.received_memos();
                    let self_send_kind = if transaction.is_orchard_to_ironwood_migration() {
                        SelfSendValueTransfer::Migration
                    } else if !memos.is_empty() {
                        SelfSendValueTransfer::MemoToSelf
                    } else {
                        SelfSendValueTransfer::Basic
                    };
                    value_transfers.push(self_send_value_transfer(
                        &transaction,
                        self_send_kind,
                        transaction.self_received_value(),
                        memos,
                    ));

                    // in the case Zennies For Zingo! is active
                    value_transfers.append(&mut create_send_value_transfers(self, &transaction)?);
                }
                TransactionKind::Received => {
                    // create 1 received value transfer for each pool received to
                    value_transfers.extend(shielded_pool_value_transfers(
                        &transaction,
                        ValueTransferKind::Received,
                    ));
                    if !transaction.transparent_coins.is_empty() {
                        let value: u64 = transaction
                            .transparent_coins
                            .iter()
                            .map(|output| output.value)
                            .sum();
                        value_transfers.push(ValueTransfer::from_summary(
                            &transaction,
                            ValueTransferKind::Received,
                            value,
                            None,
                            vec![PoolType::TRANSPARENT],
                            Vec::new(),
                        ));
                    }
                }
            }
        }

        Ok(ValueTransfers::new(value_transfers))
    }

    /// Provides a list of `ValueTransfers` associated with the sender, or containing the string.
    pub async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError> {
        let mut value_transfers = self.value_transfers(true).await?;
        value_transfers.reverse();

        // Filter out VTs where all memos are empty.
        value_transfers.retain(|vt| vt.memos.iter().any(|memo| !memo.is_empty()));

        match filter {
            Some(s) => {
                value_transfers.retain(|vt| {
                    if vt.memos.is_empty() {
                        return false;
                    }

                    if vt.recipient_address == Some(s.to_string()) {
                        true
                    } else {
                        for memo in &vt.memos {
                            if memo.contains(s) {
                                return true;
                            }
                        }
                        false
                    }
                });
            }
            None => value_transfers.retain(|vt| !vt.memos.is_empty()),
        }

        Ok(value_transfers)
    }

    /// Every finsight rollup from one derivation of the value transfers.
    pub async fn finsight(&self) -> Result<Finsight, SummaryError> {
        let value_transfers = self.value_transfers(false).await?;
        Ok(finsight_from(&value_transfers))
    }

    /// The total memo bytes sent to each recipient address, rolled up over
    /// the wallet's external sends.
    pub async fn do_total_memobytes_to_address(
        &self,
    ) -> Result<TotalMemoBytesToAddress, SummaryError> {
        Ok(self.finsight().await?.total_memobytes_to_address)
    }

    /// The number of external sends the wallet has made to each recipient
    /// address.
    pub async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError> {
        Ok(self.finsight().await?.values_sent_to_address.total_sends())
    }

    /// The total value sent to each recipient address, rolled up over the
    /// wallet's external sends.
    pub async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError> {
        Ok(self.finsight().await?.values_sent_to_address.total_value())
    }
}

/// The editorial reductions over [`LightClient`]: each method locks the
/// wallet and delegates to the [`LightWallet`] method of the same name.
impl LightClient {
    /// Wrapper for [`LightWallet::value_transfers`].
    pub async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet()
            .read()
            .await
            .value_transfers(sort_highest_to_lowest)
            .await
    }

    /// Wrapper for [`LightWallet::messages_containing`].
    pub async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet().read().await.messages_containing(filter).await
    }

    /// Wrapper for [`LightWallet::finsight`].
    pub async fn finsight(&self) -> Result<Finsight, SummaryError> {
        self.wallet().read().await.finsight().await
    }

    /// Wrapper for [`LightWallet::do_total_memobytes_to_address`].
    pub async fn do_total_memobytes_to_address(
        &self,
    ) -> Result<TotalMemoBytesToAddress, SummaryError> {
        self.wallet()
            .read()
            .await
            .do_total_memobytes_to_address()
            .await
    }

    /// Wrapper for [`LightWallet::do_total_spends_to_address`].
    pub async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError> {
        self.wallet()
            .read()
            .await
            .do_total_spends_to_address()
            .await
    }

    /// Wrapper for [`LightWallet::do_total_value_to_address`].
    pub async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError> {
        self.wallet().read().await.do_total_value_to_address().await
    }
}

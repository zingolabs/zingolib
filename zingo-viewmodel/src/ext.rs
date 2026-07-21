//! The extension traits consumers opt into.
//!
//! Method names and signatures are exactly those zingolib's wallet and
//! light client carried before the extraction, so an existing consumer
//! migrates by adding a `use` of the trait — no call site changes.

use std::collections::{HashMap, HashSet};

use zcash_protocol::PoolType;

use zingolib::lightclient::LightClient;
use zingolib::wallet::LightWallet;
use zingolib::wallet::error::{KeyError, SummaryError};
use zingolib::wallet::summary::data::{
    OutgoingNoteSummary, Scope, SendType, TransactionKind, TransactionSummary, pools_present,
};

use crate::finsight::{
    TotalMemoBytesToAddress, TotalSendsToAddress, TotalValueToAddress, ValuesSentToAddress,
};
use crate::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransfer, ValueTransferKind, ValueTransfers,
};

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
        let outgoing_coins_to_address: Vec<&zingolib::wallet::summary::data::OutgoingCoinSummary> =
            outgoing_coins
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

/// The editorial reductions over a [`LightWallet`], with the method names
/// and signatures the wallet itself carried before the extraction.
#[allow(async_fn_in_trait)] // single concrete impl; auto traits leak from it
pub trait LightWalletViewModelExt {
    /// Provides a list of value transfers related to this capability.
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError>;

    /// Provides a list of `ValueTransfers` associated with the sender, or containing the string.
    async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError>;

    /// The total outgoing-memo bytes sent to each recipient address.
    async fn do_total_memobytes_to_address(&self) -> Result<TotalMemoBytesToAddress, SummaryError>;

    /// The number of sends to each recipient address.
    async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError>;

    /// The total value sent to each recipient address.
    async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError>;
}

impl LightWalletViewModelExt for LightWallet {
    async fn value_transfers(
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
                        value_transfers.push(ValueTransfer::from_summary(
                            &transaction,
                            ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                SelfSendValueTransfer::MemoToSelf,
                            )),
                            0,
                            None,
                            transaction.pools_received(),
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
                    // create 1 memo-to-self if a sending transaction receives any number of memos
                    // otherwise, create 1 send-to-self value transfer so every transaction creates at least 1 value transfer
                    // eventually we may replace send-to-self with a range of kinds such as deshield and migrate etc.
                    let memos = transaction.received_memos();
                    let self_send_kind = if memos.is_empty() {
                        SelfSendValueTransfer::Basic
                    } else {
                        SelfSendValueTransfer::MemoToSelf
                    };
                    value_transfers.push(ValueTransfer::from_summary(
                        &transaction,
                        ValueTransferKind::Sent(SentValueTransfer::SendToSelf(self_send_kind)),
                        0,
                        None,
                        transaction.pools_received(),
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

    async fn messages_containing(
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

    async fn do_total_memobytes_to_address(&self) -> Result<TotalMemoBytesToAddress, SummaryError> {
        let value_transfers = self.value_transfers(true).await?;
        let mut memobytes_by_address = HashMap::new();
        for value_transfer in &value_transfers {
            if let ValueTransferKind::Sent(SentValueTransfer::Send) = value_transfer.kind {
                let address = value_transfer
                    .recipient_address
                    .clone()
                    .expect("sent value transfer should always have a recipient_address");
                let bytes = value_transfer.memos.iter().fold(0, |sum, m| sum + m.len());
                memobytes_by_address
                    .entry(address)
                    .and_modify(|e| *e += bytes)
                    .or_insert(bytes);
            }
        }
        Ok(TotalMemoBytesToAddress(memobytes_by_address))
    }

    async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError> {
        let values_sent_to_addresses = value_transfer_by_to_address(self).await?;
        let mut by_address_number_sends = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let number_sends = values_sent_to_addresses.0[key].len() as u64;
            by_address_number_sends.insert(key.clone(), number_sends);
        }

        Ok(TotalSendsToAddress(by_address_number_sends))
    }

    async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError> {
        let values_sent_to_addresses = value_transfer_by_to_address(self).await?;
        let mut by_address_total = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let sum = values_sent_to_addresses.0[key].iter().sum();
            by_address_total.insert(key.clone(), sum);
        }

        Ok(TotalValueToAddress(by_address_total))
    }
}

async fn value_transfer_by_to_address(
    wallet: &LightWallet,
) -> Result<ValuesSentToAddress, SummaryError> {
    let value_transfers = wallet.value_transfers(false).await?;
    let mut amount_by_address = HashMap::new();
    for value_transfer in &value_transfers {
        if let ValueTransferKind::Sent(SentValueTransfer::Send) = value_transfer.kind {
            let address = value_transfer
                .recipient_address
                .clone()
                .expect("sent value transfer should always have a recipient_address");
            amount_by_address
                .entry(address)
                .and_modify(|e: &mut Vec<u64>| e.push(value_transfer.value))
                .or_insert(vec![value_transfer.value]);
        }
    }

    Ok(ValuesSentToAddress(amount_by_address))
}

/// The editorial reductions over a [`LightClient`], with the method names
/// and signatures the client itself carried before the extraction. Each
/// method takes the wallet read lock and delegates to
/// [`LightWalletViewModelExt`].
#[allow(async_fn_in_trait)] // single concrete impl; auto traits leak from it
pub trait LightClientViewModelExt {
    /// Wrapper for [`LightWalletViewModelExt::value_transfers`].
    async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError>;

    /// Wrapper for [`LightWalletViewModelExt::messages_containing`].
    async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError>;

    /// Wrapper for [`LightWalletViewModelExt::do_total_memobytes_to_address`].
    async fn do_total_memobytes_to_address(&self) -> Result<TotalMemoBytesToAddress, SummaryError>;

    /// Wrapper for [`LightWalletViewModelExt::do_total_spends_to_address`].
    async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError>;

    /// Wrapper for [`LightWalletViewModelExt::do_total_value_to_address`].
    async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError>;
}

impl LightClientViewModelExt for LightClient {
    async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet()
            .read()
            .await
            .value_transfers(sort_highest_to_lowest)
            .await
    }

    async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet().read().await.messages_containing(filter).await
    }

    async fn do_total_memobytes_to_address(&self) -> Result<TotalMemoBytesToAddress, SummaryError> {
        self.wallet()
            .read()
            .await
            .do_total_memobytes_to_address()
            .await
    }

    async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError> {
        self.wallet()
            .read()
            .await
            .do_total_spends_to_address()
            .await
    }

    async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError> {
        self.wallet().read().await.do_total_value_to_address().await
    }
}

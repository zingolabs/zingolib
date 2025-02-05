//! Wallet-State reporters as LightWallet methods.
use json::JsonValue;
use zcash_address::ZcashAddress;
use zcash_client_backend::PoolType;
use zcash_client_backend::ShieldedProtocol;

use zcash_keys::encoding::encode_payment_address;
use zcash_primitives::consensus::NetworkConstants as _;
use zcash_primitives::consensus::Parameters;
use zcash_primitives::legacy::TransparentAddress;
use zcash_primitives::memo::Memo;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zingo_sync::primitives::OrchardNote;
use zingo_sync::primitives::OutgoingNoteInterface;
use zingo_sync::primitives::OutputInterface;
use zingo_sync::primitives::SaplingNote;
use zingo_sync::primitives::TransparentCoin;
use zingo_sync::primitives::WalletTransaction;

use std::cmp::Ordering;

use bip0039::Mnemonic;

use zcash_note_encryption::Domain;

use crate::config::ChainType;
use crate::config::ZENNIES_FOR_ZINGO_DONATION_ADDRESS;
use crate::config::ZENNIES_FOR_ZINGO_REGTEST_ADDRESS;
use crate::config::ZENNIES_FOR_ZINGO_TESTNET_ADDRESS;
use crate::utils;
use crate::wallet::notes::ShieldedNoteInterface;

use crate::wallet::traits::Diversifiable as _;

use crate::wallet::error::BalanceError;
use crate::wallet::keys::unified::WalletCapability;
use crate::wallet::traits::DomainWalletExt;
use crate::wallet::traits::Recipient;

use crate::wallet::LightWallet;
use crate::Orchard;
use crate::Sapling;
use crate::UAReceivers;
use crate::WalletDomain;

use super::data::summaries::NoteSummary;
use super::data::summaries::OutgoingNoteSummary;
use super::data::summaries::SelfSendValueTransfer;
use super::data::summaries::SentValueTransfer;
use super::data::summaries::TransactionSummaries;
use super::data::summaries::TransactionSummary;
use super::data::summaries::TransactionSummaryBuilder;
use super::data::summaries::TransactionSummaryInterface as _;
use super::data::summaries::TransparentCoinSummary;
use super::data::summaries::ValueTransfer;
use super::data::summaries::ValueTransferBuilder;
use super::data::summaries::ValueTransferKind;
use super::data::summaries::ValueTransfers;
use super::keys::unified::UnifiedKeyStore;
use super::transaction_record::SendType;
use super::transaction_record::TransactionKind;

impl LightWallet {
    /// Returns wallet addresses in a JSON array
    pub async fn do_addresses(&self, subset: UAReceivers) -> JsonValue {
        let mut objectified_addresses = Vec::new();
        for address in self.unified_addresses.iter() {
            let local_address = match subset {
                UAReceivers::Orchard => zcash_keys::address::UnifiedAddress::from_receivers(
                    address.orchard().copied(),
                    None,
                    None,
                )
                .expect("To create a new address."),
                UAReceivers::Shielded => zcash_keys::address::UnifiedAddress::from_receivers(
                    address.orchard().copied(),
                    address.sapling().copied(),
                    None,
                )
                .expect("To create a new address."),
                UAReceivers::All => address.clone(),
            };
            let encoded_ua = local_address.encode(&self.network);
            let transparent = local_address
                .transparent()
                .map(|taddr| zingo_sync::keys::transparent::encode_address(&self.network, *taddr));
            objectified_addresses.push(
                json::object!{
                    "address" => encoded_ua,
                    "receivers" => json::object!(
                        "transparent" => transparent,
                        "sapling" => local_address.sapling().map(|z_addr| encode_payment_address(self.network.hrp_sapling_payment_address(), z_addr)),
                        "orchard_exists" => local_address.orchard().is_some()
                    )
                }
            )
        }
        JsonValue::Array(objectified_addresses)

        // let mut objectified_addresses = Vec::new();
        // for address in self.unified_addresses.iter() {
        //     let encoded_ua = address.encode(&self.network);
        //     let transparent = address
        //         .transparent()
        //         .map(|taddr| zingo_sync::keys::transparent::encode_address(&self.network, *taddr));
        //     objectified_addresses.push(json::object! {
        // "address" => encoded_ua,
        // "receivers" => json::object!(
        //     "transparent" => transparent,
        //     "sapling" => address.sapling().map(|z_addr| zcash_keys::encoding::encode_payment_address(self.network.hrp_sapling_payment_address(), z_addr)),
        //     "orchard_exists" => address.orchard().is_some(),
        //     )
        // })
        // }
        // JsonValue::Array(objectified_addresses)
    }

    /// returns Some seed phrase for the wallet.
    /// if wallet does not have a seed phrase, returns None
    pub async fn get_seed_phrase(&self) -> Option<String> {
        self.mnemonic()
            .map(|(mnemonic, _)| mnemonic.phrase().to_string())
    }

    /// Returns the total balance of the all outputs of a given pool type in the wallet matching the output and transaction
    /// criteria specified by the `filter_function`.
    /// Returns `None` if the wallet does not have viewing capability for the given pool type.
    pub async fn get_filtered_balance<D, F>(&self, filter_function: F) -> Option<u64>
    where
        D: WalletDomain,
        F: Fn(&D::Output, &WalletTransaction) -> bool,
    {
        match &self.unified_key_store {
            UnifiedKeyStore::Spend(_) => (),
            UnifiedKeyStore::View(ufvk) => match D::POOL_TYPE {
                PoolType::Transparent => {
                    ufvk.transparent()?;
                }
                PoolType::Shielded(ShieldedProtocol::Sapling) => {
                    ufvk.sapling()?;
                }
                PoolType::Shielded(ShieldedProtocol::Orchard) => {
                    ufvk.orchard()?;
                }
            },
            UnifiedKeyStore::Empty => return None,
        }

        Some(
            self.wallet_transactions
                .values()
                .fold(0, |acc, transaction| {
                    acc + D::Output::transaction_outputs(transaction)
                        .iter()
                        .filter(|&note| {
                            filter_function(note, transaction)
                                && note.spending_transaction().is_none()
                        })
                        .map(|output| output.value())
                        .sum::<u64>()
                }),
        )
    }

    /// Returns total wallet balance of unspent notes in confirmed blocks for a given shielded pool.
    pub async fn confirmed_balance<D>(&self) -> Option<u64>
    where
        D: WalletDomain,
    {
        self.get_filtered_balance::<D, _>(|_, transaction: &WalletTransaction| {
            transaction.status().is_confirmed()
        })
        .await
    }

    /// Returns total wallet balance of unspent notes in confirmed blocks for a given shielded pool.
    /// Returns `None` if the wallet does not have spend capability.
    // TODO: also calculate whether notes in the wallet have the necessary info (i.e. commitment trees) to spend
    pub async fn spendable_balance<D>(&self) -> Option<u64>
    where
        D: WalletDomain,
    {
        if let UnifiedKeyStore::Spend(_) = self.unified_key_store {
            self.confirmed_balance::<D>().await
        } else {
            None
        }
    }

    /// Returns total wallet balance of unspent notes not yet confirmed on the block chain for a given shielded pool.
    pub async fn pending_balance<D>(&self) -> Option<u64>
    where
        D: WalletDomain,
    {
        self.get_filtered_balance::<D, _>(|_, transaction: &WalletTransaction| {
            !transaction.status().is_confirmed()
        })
        .await
    }

    /// Returns total wallet balance of unspent notes in confirmed blocks for a given shielded pool excluding any notes
    /// with value less than marginal fee (5_000).
    pub async fn confirmed_balance_excluding_dust<D>(&self) -> Option<u64>
    where
        D: WalletDomain,
    {
        self.get_filtered_balance::<D, _>(|note, transaction: &WalletTransaction| {
            D::Output::value(note) > MARGINAL_FEE.into_u64() && transaction.status().is_confirmed()
        })
        .await
    }

    /// Returns total balance of all shielded pools excluding any notes with value less than marginal fee
    /// that are confirmed on the block chain (the block has at least 1 confirmation).
    /// Does not include transparent funds.
    ///
    /// # Error
    ///
    /// Returns an error if the full viewing key is not found or if the balance summation exceeds the valid range of zatoshis.
    pub async fn confirmed_shielded_balance_excluding_dust(
        &self,
    ) -> Result<NonNegativeAmount, BalanceError> {
        Ok(utils::conversion::zatoshis_from_u64(
            self.confirmed_balance_excluding_dust::<Orchard>()
                .await
                .ok_or(BalanceError::NoFullViewingKey)?
                + self
                    .confirmed_balance_excluding_dust::<Sapling>()
                    .await
                    .ok_or(BalanceError::NoFullViewingKey)?,
        )?)
    }

    /// TODO: Add Doc Comment Here!
    // FIXME: zingo2
    #[allow(dead_code)]
    pub(crate) fn note_address<D: DomainWalletExt>(
        network: &crate::config::ChainType,
        note: &D::WalletNote,
        wallet_capability: &WalletCapability,
    ) -> String
    where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        D::unified_key_store_to_fvk(&wallet_capability.unified_key_store).expect("to get fvk from the unified key store")
        .diversified_address(*note.diversifier())
        .and_then(|address| {
            D::ua_from_contained_receiver(wallet_capability, &address)
                .map(|ua| ua.encode(network))
        })
        .unwrap_or("Diversifier not in wallet. Perhaps you restored from seed and didn't restore addresses".to_string())
    }

    /// TODO: Add Doc Comment Here!
    pub fn mnemonic(&self) -> Option<&(Mnemonic, u32)> {
        self.mnemonic.as_ref()
    }

    /// lists the transparent addresses known by the wallet.
    pub fn get_transparent_addresses(&self) -> Vec<TransparentAddress> {
        self.transparent_addresses
            .values()
            .map(|address| {
                ZcashAddress::try_from_encoded(&address)
                    .unwrap()
                    .convert_if_network::<TransparentAddress>(self.network.network_type())
                    .expect("incorrect network should be checked on wallet load")
            })
            .collect()
    }

    /// Note this method is INCORRECT in the case of a 0-value, 0-fee transaction from the
    /// Creating Capability.  Such a transaction would violate ZIP317, but could exist in
    /// the Zcash protocol
    ///  TODO:   Test and handle 0-value, 0-fee transaction
    pub(crate) fn transaction_kind(&self, transaction: &WalletTransaction) -> TransactionKind {
        let zfz_address = match self.network {
            ChainType::Mainnet => ZENNIES_FOR_ZINGO_DONATION_ADDRESS,
            ChainType::Testnet => ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
            ChainType::Regtest(_) => ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
        };

        let transparent_spends = self
            .find_spends::<TransparentCoin>(transaction, false)
            .expect("fail_on_miss is set false");
        let sapling_spends = self
            .find_spends::<SaplingNote>(transaction, false)
            .expect("fail_on_miss is set false");
        let orchard_spends = self
            .find_spends::<OrchardNote>(transaction, false)
            .expect("fail_on_miss is set false");

        if transparent_spends.is_empty()
            && sapling_spends.is_empty()
            && orchard_spends.is_empty()
            && transaction.outgoing_sapling_notes().is_empty()
            && transaction.outgoing_orchard_notes().is_empty()
        {
            // no spends and no outgoing notes
            TransactionKind::Received
        } else if !transparent_spends.is_empty()
            && sapling_spends.is_empty()
            && orchard_spends.is_empty()
            && transaction.outgoing_sapling_notes().is_empty()
            && transaction.outgoing_orchard_notes().is_empty()
            && (!transaction.orchard_notes().is_empty() | !transaction.sapling_notes().is_empty())
        {
            // only transparent spends, no outgoing notes and notes received
            // TODO: this could be improved by checking outputs recipient addr against the wallet addrs
            TransactionKind::Sent(SendType::Shield)
        } else if transaction.outgoing_sapling_notes().is_empty()
            && transaction.outgoing_orchard_notes().is_empty()
            || (transaction.outgoing_sapling_notes().len() == 1
                && transaction
                    .outgoing_sapling_notes()
                    .iter()
                    .any(|outgoing_note| {
                        outgoing_note.encoded_recipient(&self.network) == zfz_address.to_string()
                            || outgoing_note.encoded_recipient_unified_address(&self.network)
                                == Some(zfz_address.to_string())
                    }))
            || (transaction.outgoing_orchard_notes().len() == 1
                && transaction
                    .outgoing_orchard_notes()
                    .iter()
                    .any(|outgoing_note| {
                        outgoing_note.encoded_recipient(&self.network) == zfz_address.to_string()
                            || outgoing_note.encoded_recipient_unified_address(&self.network)
                                == Some(zfz_address.to_string())
                    }))
        {
            // not Received, this capability created this transaction
            // not Shield, notes were spent
            // no outgoing notes, with the exception of ONLY a Zennies For Zingo! donation
            TransactionKind::Sent(SendType::SendToSelf)
        } else {
            TransactionKind::Sent(SendType::Send)
        }
    }

    /// Provides a list of transaction summaries related to this wallet in order of blockheight
    pub async fn transaction_summaries(&self) -> TransactionSummaries {
        let mut transaction_summaries = self
            .wallet_transactions
            .values()
            .map(|transaction| {
                let (
                    kind,
                    value,
                    fee,
                    orchard_notes,
                    sapling_notes,
                    transparent_coins,
                    outgoing_sapling_notes,
                    outgoing_orchard_notes,
                ) = self.basic_transaction_summary_parts(transaction);

                TransactionSummaryBuilder::new()
                    .txid(transaction.txid())
                    .datetime(transaction.datetime())
                    .blockheight(transaction.status().get_height())
                    .kind(kind)
                    .value(value)
                    .fee(fee)
                    .status(transaction.status())
                    .zec_price(transaction.price())
                    .transparent_coins(transparent_coins)
                    .sapling_notes(sapling_notes)
                    .orchard_notes(orchard_notes)
                    .outgoing_sapling_notes(outgoing_sapling_notes)
                    .outgoing_orchard_notes(outgoing_orchard_notes)
                    .build()
                    .expect("all fields should be populated")
            })
            .collect::<Vec<_>>();

        transaction_summaries.sort_by(|summary_a, summary_b| {
            match summary_a.blockheight().cmp(&summary_b.blockheight()) {
                Ordering::Equal => {
                    let starts_with_tex = |summary: &TransactionSummary| {
                        summary
                            .outgoing_sapling_notes()
                            .iter()
                            .chain(summary.outgoing_orchard_notes().iter())
                            .any(|outgoing_note| outgoing_note.recipient.starts_with("tex"))
                    };
                    match (starts_with_tex(summary_a), starts_with_tex(summary_b)) {
                        (true, false) => Ordering::Greater,
                        (false, true) => Ordering::Less,
                        (false, false) | (true, true) => Ordering::Equal,
                    }
                }
                otherwise => otherwise,
            }
        });

        TransactionSummaries::new(transaction_summaries)
    }

    /// TODO: doc comment
    pub async fn transaction_summaries_json_string(&self) -> String {
        json::JsonValue::from(self.transaction_summaries().await).pretty(2)
    }

    fn basic_transaction_summary_parts(
        &self,
        transaction: &WalletTransaction,
    ) -> (
        TransactionKind,
        u64,
        Option<u64>,
        Vec<NoteSummary>,
        Vec<NoteSummary>,
        Vec<TransparentCoinSummary>,
        Vec<OutgoingNoteSummary>,
        Vec<OutgoingNoteSummary>,
    ) {
        let kind = self.transaction_kind(transaction);
        let value = match kind {
            TransactionKind::Received
            | TransactionKind::Sent(SendType::Shield)
            | TransactionKind::Sent(SendType::SendToSelf) => transaction.total_value_received(),
            TransactionKind::Sent(SendType::Send) => transaction.total_value_sent(),
        };
        let fee = self.calculate_transaction_fee(transaction).ok();
        let orchard_notes = transaction
            .orchard_notes()
            .iter()
            .map(|output| {
                let spend_status = self.output_spend_status(output);

                let memo = if let Memo::Text(memo_text) = &output.memo {
                    Some(memo_text.to_string())
                } else {
                    None
                };

                NoteSummary::from_parts(
                    output.value(),
                    spend_status,
                    output.output_id().output_index() as u32,
                    memo,
                )
            })
            .collect::<Vec<_>>();
        let sapling_notes = transaction
            .sapling_notes()
            .iter()
            .map(|output| {
                let spend_status = self.output_spend_status(output);

                let memo = if let Memo::Text(memo_text) = &output.memo {
                    Some(memo_text.to_string())
                } else {
                    None
                };

                NoteSummary::from_parts(
                    output.value(),
                    spend_status,
                    output.output_id().output_index() as u32,
                    memo,
                )
            })
            .collect::<Vec<_>>();
        let transparent_coins = transaction
            .transparent_coins()
            .iter()
            .map(|output| {
                let spend_status = self.output_spend_status(output);

                TransparentCoinSummary::from_parts(
                    output.value(),
                    spend_status,
                    output.output_id().output_index() as u32,
                )
            })
            .collect::<Vec<_>>();

        let outgoing_orchard_notes = transaction
            .outgoing_orchard_notes()
            .iter()
            .map(|note| {
                let memo = if let Memo::Text(memo_text) = note.memo() {
                    Some(memo_text.to_string())
                } else {
                    None
                };

                OutgoingNoteSummary {
                    output_index: note.output_id().output_index(),
                    key_id: note.key_id(),
                    memo,
                    value: note.value(),
                    recipient: note.encoded_recipient(&self.network),
                    recipient_unified_address: note
                        .encoded_recipient_unified_address(&self.network),
                }
            })
            .collect::<Vec<_>>();
        let outgoing_sapling_notes = transaction
            .outgoing_sapling_notes()
            .iter()
            .map(|note| {
                let memo = if let Memo::Text(memo_text) = note.memo() {
                    Some(memo_text.to_string())
                } else {
                    None
                };

                OutgoingNoteSummary {
                    output_index: note.output_id().output_index(),
                    key_id: note.key_id(),
                    memo,
                    value: note.value(),
                    recipient: note.encoded_recipient(&self.network),
                    recipient_unified_address: note
                        .encoded_recipient_unified_address(&self.network),
                }
            })
            .collect::<Vec<_>>();
        (
            kind,
            value,
            fee,
            orchard_notes,
            sapling_notes,
            transparent_coins,
            outgoing_orchard_notes,
            outgoing_sapling_notes,
        )
    }

    /// Provides a list of value transfers related to this capability
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    pub async fn value_transfers(&self) -> ValueTransfers {
        let mut value_transfers: Vec<ValueTransfer> = Vec::new();
        let summaries = self.transaction_summaries().await;

        let transaction_summaries = summaries.iter();

        for tx in transaction_summaries {
            match tx.kind() {
                TransactionKind::Sent(SendType::Send) => {
                    // create 1 sent value transfer for each non-self recipient address
                    // if recipient_ua is available it overrides recipient_address
                    value_transfers.append(&mut ValueTransfers::create_send_value_transfers(tx));

                    // create 1 memo-to-self if a sending transaction receives any number of memos
                    if tx.orchard_notes().iter().any(|note| note.memo().is_some())
                        || tx.sapling_notes().iter().any(|note| note.memo().is_some())
                    {
                        let memos: Vec<String> = tx
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .chain(
                                tx.sapling_notes()
                                    .iter()
                                    .filter_map(|note| note.memo().map(|memo| memo.to_string())),
                            )
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::MemoToSelf,
                                )))
                                .value(0)
                                .recipient_address(None)
                                .pool_received(None)
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                }
                TransactionKind::Sent(SendType::Shield) => {
                    // create 1 shielding value transfer for each pool shielded to
                    if !tx.orchard_notes().is_empty() {
                        let value: u64 =
                            tx.orchard_notes().iter().map(|output| output.value()).sum();
                        let memos: Vec<String> = tx
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::Shield,
                                )))
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(
                                    PoolType::Shielded(ShieldedProtocol::Orchard).to_string(),
                                ))
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                    if !tx.sapling_notes().is_empty() {
                        let value: u64 =
                            tx.sapling_notes().iter().map(|output| output.value()).sum();
                        let memos: Vec<String> = tx
                            .sapling_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::Shield,
                                )))
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(
                                    PoolType::Shielded(ShieldedProtocol::Sapling).to_string(),
                                ))
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                }
                TransactionKind::Sent(SendType::SendToSelf) => {
                    // create 1 memo-to-self if a sending transaction receives any number of memos
                    // otherwise, create 1 send-to-self value transfer so every transaction creates at least 1 value transfer
                    // eventually we may replace send-to-self with a range of kinds such as deshield and migrate etc.
                    if tx.orchard_notes().iter().any(|note| note.memo().is_some())
                        || tx.sapling_notes().iter().any(|note| note.memo().is_some())
                    {
                        let memos: Vec<String> = tx
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .chain(
                                tx.sapling_notes()
                                    .iter()
                                    .filter_map(|note| note.memo().map(|memo| memo.to_string())),
                            )
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::MemoToSelf,
                                )))
                                .value(0)
                                .recipient_address(None)
                                .pool_received(None)
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    } else {
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::Basic,
                                )))
                                .value(0)
                                .recipient_address(None)
                                .pool_received(None)
                                .memos(vec![])
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }

                    // in the case Zennies For Zingo! is active
                    value_transfers.append(&mut ValueTransfers::create_send_value_transfers(tx));
                }
                TransactionKind::Received => {
                    // create 1 received value transfer for each pool received to
                    if !tx.orchard_notes().is_empty() {
                        let value: u64 =
                            tx.orchard_notes().iter().map(|output| output.value()).sum();
                        let memos: Vec<String> = tx
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
                                .kind(ValueTransferKind::Received)
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(
                                    PoolType::Shielded(ShieldedProtocol::Orchard).to_string(),
                                ))
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                    if !tx.sapling_notes().is_empty() {
                        let value: u64 =
                            tx.sapling_notes().iter().map(|output| output.value()).sum();
                        let memos: Vec<String> = tx
                            .sapling_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
                                .kind(ValueTransferKind::Received)
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(
                                    PoolType::Shielded(ShieldedProtocol::Sapling).to_string(),
                                ))
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                    if !tx.transparent_coins().is_empty() {
                        let value: u64 = tx
                            .transparent_coins()
                            .iter()
                            .map(|output| output.value())
                            .sum();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
                                .kind(ValueTransferKind::Received)
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(PoolType::Transparent.to_string()))
                                .memos(Vec::new())
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                }
            };
        }
        ValueTransfers::new(value_transfers)
    }

    /// Provides a list of value transfers sorted
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    pub async fn sorted_value_transfers(&self, newer_first: bool) -> ValueTransfers {
        let mut value_transfers = self.value_transfers().await;
        if newer_first {
            value_transfers.reverse();
        }
        value_transfers
    }

    /// TODO: doc comment
    pub async fn value_transfers_json_string(&self) -> String {
        json::JsonValue::from(self.sorted_value_transfers(true).await).pretty(2)
    }

    // FIXME: zingo2, re implement
    // /// Provides a detailed list of transaction summaries related to this wallet in order of blockheight
    // pub async fn detailed_transaction_summaries(&self) -> DetailedTransactionSummaries {
    //     let wallet = self.wallet.lock().await;
    //     let transaction_map = wallet
    //         .transaction_context
    //         .transaction_metadata_set
    //         .read()
    //         .await;
    //     let transaction_records = &transaction_map.transaction_records_by_id;

    //     let mut transaction_summaries = transaction_records
    //         .values()
    //         .map(|tx| {
    //             let (kind, value, fee, orchard_notes, sapling_notes, transparent_coins) =
    //                 basic_transaction_summary_parts(tx, transaction_records, &self.config().chain);
    //             let orchard_nullifiers: Vec<String> = tx
    //                 .spent_orchard_nullifiers
    //                 .iter()
    //                 .map(|nullifier| hex::encode(nullifier.to_bytes()))
    //                 .collect();
    //             let sapling_nullifiers: Vec<String> = tx
    //                 .spent_sapling_nullifiers
    //                 .iter()
    //                 .map(hex::encode)
    //                 .collect();

    //             DetailedTransactionSummaryBuilder::new()
    //                 .txid(tx.txid)
    //                 .datetime(tx.datetime)
    //                 .blockheight(tx.status.get_height())
    //                 .kind(kind)
    //                 .value(value)
    //                 .fee(fee)
    //                 .status(tx.status)
    //                 .zec_price(tx.price)
    //                 .orchard_notes(orchard_notes)
    //                 .sapling_notes(sapling_notes)
    //                 .transparent_coins(transparent_coins)
    //                 .outgoing_tx_data(tx.outgoing_tx_data.clone())
    //                 .orchard_nullifiers(orchard_nullifiers)
    //                 .sapling_nullifiers(sapling_nullifiers)
    //                 .build()
    //                 .expect("all fields should be populated")
    //         })
    //         .collect::<Vec<_>>();
    //     drop(transaction_map);
    //     drop(wallet);

    //     transaction_summaries.sort_by_key(|tx| tx.blockheight());

    //     DetailedTransactionSummaries::new(transaction_summaries)
    // }

    // /// TODO: doc comment
    // pub async fn detailed_transaction_summaries_json_string(&self) -> String {
    //     json::JsonValue::from(self.detailed_transaction_summaries().await).pretty(2)
    // }
}

#[cfg(any(test, feature = "test-elevation"))]
mod test {

    use zcash_client_backend::PoolType;
    use zcash_client_backend::ShieldedProtocol;
    use zcash_primitives::consensus::NetworkConstants as _;

    use crate::wallet::LightWallet;

    /// these functions have clearer typing than
    /// the production functions using json that could be upgraded soon
    impl LightWallet {
        #[allow(clippy::result_unit_err)]
        /// gets a UnifiedAddress, the first of the wallet.
        /// zingolib includes derivations of further addresses.
        /// ZingoMobile uses one address.
        pub fn get_first_ua(&self) -> Result<zcash_keys::address::UnifiedAddress, ()> {
            Ok(self.unified_addresses.iter().next().ok_or(())?.clone())
        }

        #[allow(clippy::result_unit_err)]
        /// UnifiedAddress type is not a string. to process it into a string requires chain date.
        pub fn encode_ua_as_pool(
            &self,
            ua: &zcash_keys::address::UnifiedAddress,
            pool: PoolType,
        ) -> Result<String, ()> {
            match pool {
                PoolType::Transparent => ua
                    .transparent()
                    .map(|taddr| {
                        // TODO: new crate for shared conversion, parsing and encoding
                        zingo_sync::keys::transparent::encode_address(&self.network, *taddr)
                    })
                    .ok_or(()),
                PoolType::Shielded(ShieldedProtocol::Sapling) => ua
                    .sapling()
                    .map(|z_addr| {
                        zcash_keys::encoding::encode_payment_address(
                            self.network.hrp_sapling_payment_address(),
                            z_addr,
                        )
                    })
                    .ok_or(()),
                PoolType::Shielded(ShieldedProtocol::Orchard) => Ok(ua.encode(&self.network)),
            }
        }

        #[allow(clippy::result_unit_err)]
        /// gets a string address for the wallet, based on pooltype
        pub fn get_first_address(&self, pool: PoolType) -> Result<String, ()> {
            let ua = self.get_first_ua()?;
            self.encode_ua_as_pool(&ua, pool)
        }
    }

    // FIXME: zingo2
    // #[cfg(test)]
    // use crate::Orchard;
    // #[cfg(test)]
    // use crate::Sapling;
    // #[cfg(test)]
    // use zingo_status::confirmation_status::ConfirmationStatus;

    // #[cfg(test)]
    // use crate::config::ZingoConfigBuilder;
    // #[cfg(test)]
    // use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;
    // #[cfg(test)]
    // use crate::mocks::SaplingCryptoNoteBuilder;
    // #[cfg(test)]
    // use crate::wallet::notes::orchard::mocks::OrchardNoteBuilder;
    // #[cfg(test)]
    // use crate::wallet::notes::sapling::mocks::SaplingNoteBuilder;
    // #[cfg(test)]
    // use crate::wallet::notes::transparent::mocks::TransparentOutputBuilder;
    // #[cfg(test)]
    // use crate::wallet::transaction_record::mocks::TransactionRecordBuilder;
    // #[cfg(test)]
    // use crate::wallet::WalletBase;

    // FIXME: zingo2
    // #[tokio::test]
    // async fn confirmed_balance_excluding_dust() {
    //     let wallet = LightWallet::new(
    //         ZingoConfigBuilder::default().create().chain,
    //         WalletBase::FreshEntropy,
    //         1.into(),
    //     )
    //     .unwrap();
    //     let confirmed_tx_record = TransactionRecordBuilder::default()
    //         .status(ConfirmationStatus::Confirmed(80.into()))
    //         .transparent_outputs(TransparentOutputBuilder::default())
    //         .sapling_notes(SaplingNoteBuilder::default())
    //         .sapling_notes(SaplingNoteBuilder::default())
    //         .sapling_notes(
    //             SaplingNoteBuilder::default()
    //                 .note(
    //                     SaplingCryptoNoteBuilder::default()
    //                         .value(sapling_crypto::value::NoteValue::from_raw(3_000))
    //                         .clone(),
    //                 )
    //                 .clone(),
    //         )
    //         .orchard_notes(OrchardNoteBuilder::default())
    //         .orchard_notes(OrchardNoteBuilder::default())
    //         .orchard_notes(
    //             OrchardNoteBuilder::default()
    //                 .note(
    //                     OrchardCryptoNoteBuilder::default()
    //                         .value(orchard::value::NoteValue::from_raw(5_001))
    //                         .clone(),
    //                 )
    //                 .clone(),
    //         )
    //         .orchard_notes(
    //             OrchardNoteBuilder::default()
    //                 .note(
    //                     OrchardCryptoNoteBuilder::default()
    //                         .value(orchard::value::NoteValue::from_raw(2_000))
    //                         .clone(),
    //                 )
    //                 .clone(),
    //         )
    //         .build();
    //     let mempool_tx_record = TransactionRecordBuilder::default()
    //         .status(ConfirmationStatus::Mempool(95.into()))
    //         .transparent_outputs(TransparentOutputBuilder::default())
    //         .sapling_notes(SaplingNoteBuilder::default())
    //         .orchard_notes(OrchardNoteBuilder::default())
    //         .build();
    //     {
    //         let mut tx_map = wallet
    //             .transaction_context
    //             .transaction_metadata_set
    //             .write()
    //             .await;
    //         tx_map
    //             .transaction_records_by_id
    //             .insert_transaction_record(confirmed_tx_record);
    //         tx_map
    //             .transaction_records_by_id
    //             .insert_transaction_record(mempool_tx_record);
    //     }

    //     assert_eq!(
    //         wallet.confirmed_balance_excluding_dust::<Sapling>().await,
    //         Some(400_000)
    //     );
    //     assert_eq!(
    //         wallet.confirmed_balance_excluding_dust::<Orchard>().await,
    //         Some(1_605_001)
    //     );
    // }
}

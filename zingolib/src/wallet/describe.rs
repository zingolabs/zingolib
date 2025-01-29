//! Wallet-State reporters as LightWallet methods.
use zcash_client_backend::ShieldedProtocol;

use zcash_primitives::transaction::components::amount::NonNegativeAmount;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zingo_sync::primitives::WalletTransaction;

use std::sync::Arc;
use tokio::sync::RwLock;

use bip0039::Mnemonic;

use zcash_note_encryption::Domain;

use crate::utils;
use crate::wallet::notes::OutputInterface;
use crate::wallet::notes::ShieldedNoteInterface;

use crate::wallet::traits::Diversifiable as _;

use crate::wallet::error::BalanceError;
use crate::wallet::keys::unified::WalletCapability;
use crate::wallet::notes::TransparentOutput;
use crate::wallet::traits::DomainWalletExt;
use crate::wallet::traits::Recipient;

use crate::wallet::tx_map::TxMap;
use crate::wallet::LightWallet;
use crate::Orchard;
use crate::Sapling;
use crate::WalletDomain;

use super::data::summaries::OrchardNoteSummary;
use super::data::summaries::SaplingNoteSummary;
use super::data::summaries::TransactionSummaries;
use super::data::summaries::TransactionSummaryBuilder;
use super::data::summaries::TransparentCoinSummary;
use super::keys::unified::UnifiedKeyStore;
use super::transaction_record::TransactionKind;

impl LightWallet {
    /// returns Some seed phrase for the wallet.
    /// if wallet does not have a seed phrase, returns None
    pub async fn get_seed_phrase(&self) -> Option<String> {
        self.mnemonic()
            .map(|(mnemonic, _)| mnemonic.phrase().to_string())
    }

    // Core shielded_balance function, other public methods dispatch specific sets of filters to this
    // method for processing.
    /// Returns the sum of unspent notes recorded by the wallet with optional filtering.
    /// This method ensures that `None` is returned in the case of a missing view capability.
    pub async fn get_filtered_balance<D, F>(&self, filter_function: F) -> Option<u64>
    where
        D: WalletDomain,
        F: Fn(&D::Note, &WalletTransaction) -> bool,
    {
        match &self.unified_key_store {
            UnifiedKeyStore::Spend(_) => (),
            UnifiedKeyStore::View(ufvk) => match D::SHIELDED_PROTOCOL {
                ShieldedProtocol::Sapling => {
                    ufvk.sapling()?;
                }
                ShieldedProtocol::Orchard => {
                    ufvk.orchard()?;
                }
            },
            UnifiedKeyStore::Empty => return None,
        }

        Some(
            self.wallet_transactions
                .values()
                .fold(0, |acc, transaction| {
                    acc + D::Note::transaction_notes(transaction)
                        .iter()
                        .filter(|&note| {
                            filter_function(note, transaction)
                                && note.spending_transaction().is_none()
                        })
                        .map(|note| note.value())
                        .sum::<u64>()
                }),
        )
    }

    /// Sums the transparent balance (unspent)
    pub async fn get_transparent_balance(&self) -> Option<u64> {
        match &self.unified_key_store {
            UnifiedKeyStore::Spend(_) => (),
            UnifiedKeyStore::View(ufvk) => {
                ufvk.transparent()?;
            }
            UnifiedKeyStore::Empty => return None,
        }
        Some(
            self.get_utxos()
                .await
                .iter()
                .filter(|transparent_output| transparent_output.is_unspent())
                .map(|utxo| utxo.value)
                .sum::<u64>(),
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
            D::Note::value(note) > MARGINAL_FEE.into_u64() && transaction.status().is_confirmed()
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

    /// Get all (unspent) utxos.
    pub async fn get_utxos(&self) -> Vec<TransparentOutput> {
        self.transaction_context
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id
            .values()
            .flat_map(|transaction| {
                transaction
                    .transparent_outputs
                    .iter()
                    .filter(|utxo| !utxo.is_spent_confirmed())
            })
            .cloned()
            .collect::<Vec<TransparentOutput>>()
    }

    /// TODO: Add Doc Comment Here!
    pub fn transactions(&self) -> Arc<RwLock<TxMap>> {
        self.transaction_context.transaction_metadata_set.clone()
    }

    /// lists the transparent addresses known by the wallet.
    pub fn get_transparent_addresses(&self) -> Vec<zcash_primitives::legacy::TransparentAddress> {
        self.wallet_capability()
            .transparent_child_addresses()
            .iter()
            .map(|(_index, sk)| *sk)
            .collect::<Vec<_>>()
    }

    /// Provides a list of transaction summaries related to this wallet in order of blockheight
    pub async fn transaction_summaries(&self) -> TransactionSummaries {
        let mut transaction_summaries = self
            .wallet_transactions
            .values()
            .map(|transaction| {
                let (kind, value, fee, orchard_notes, sapling_notes, transparent_coins) =
                    self.basic_transaction_summary_parts(transaction);

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
                    .outgoing_tx_data(transaction.outgoing_tx_data.clone())
                    .build()
                    .expect("all fields should be populated")
            })
            .collect::<Vec<_>>();
        drop(transaction_map);
        drop(wallet);

        transaction_summaries.sort_by(|sum1, sum2| {
            match sum1.blockheight().cmp(&sum2.blockheight()) {
                Ordering::Equal => {
                    let starts_with_tex = |summary: &TransactionSummary| {
                        summary.outgoing_tx_data().iter().any(|outgoing_txdata| {
                            outgoing_txdata.recipient_address.starts_with("tex")
                        })
                    };
                    match (starts_with_tex(sum1), starts_with_tex(sum2)) {
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
                // example of creating spend summary "from foundational truths" with correct wallet level insight
                let spend_summary = if let Some(status) = wallet.output_spend_status(output) {
                    match status {
                        ConfirmationStatus::Confirmed(_) => SpendSummary::Spent(
                            output
                                .spending_txid()
                                .expect("transaction must exist in the wallet"),
                        ),
                        ConfirmationStatus::Transmitted(_) => SpendSummary::TransmittedSpent(
                            output
                                .spending_txid()
                                .expect("transaction must exist in the wallet"),
                        ),
                        ConfirmationStatus::Mempool(_) => SpendSummary::MempoolSpent(
                            output
                                .spending_txid()
                                .expect("transaction must exist in the wallet"),
                        ),
                        _ => SpendSummary::Unspent, // TODO: add calculated spent
                    }
                } else {
                    SpendSummary::Unspent
                };

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

#[cfg(any(test, feature = "test-elevation"))]
mod test {

    use zcash_client_backend::PoolType;
    use zcash_client_backend::ShieldedProtocol;

    use crate::wallet::LightWallet;

    /// these functions have clearer typing than
    /// the production functions using json that could be upgraded soon
    impl LightWallet {
        #[allow(clippy::result_unit_err)]
        /// gets a UnifiedAddress, the first of the wallet.
        /// zingolib includes derivations of further addresses.
        /// ZingoMobile uses one address.
        pub fn get_first_ua(&self) -> Result<zcash_keys::address::UnifiedAddress, ()> {
            Ok(self
                .wallet_capability()
                .addresses()
                .iter()
                .next()
                .ok_or(())?
                .clone())
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
                        crate::wallet::keys::address_from_pubkeyhash(
                            &self.transaction_context.config,
                            *taddr,
                        )
                    })
                    .ok_or(()),
                PoolType::Shielded(ShieldedProtocol::Sapling) => ua
                    .sapling()
                    .map(|z_addr| {
                        zcash_keys::encoding::encode_payment_address(
                            self.transaction_context
                                .config
                                .chain
                                .hrp_sapling_payment_address(),
                            z_addr,
                        )
                    })
                    .ok_or(()),
                PoolType::Shielded(ShieldedProtocol::Orchard) => {
                    Ok(ua.encode(&self.transaction_context.config.chain))
                }
            }
        }

        #[allow(clippy::result_unit_err)]
        /// gets a string address for the wallet, based on pooltype
        pub fn get_first_address(&self, pool: PoolType) -> Result<String, ()> {
            let ua = self.get_first_ua()?;
            self.encode_ua_as_pool(&ua, pool)
        }
    }

    #[cfg(test)]
    use crate::Orchard;
    #[cfg(test)]
    use crate::Sapling;
    #[cfg(test)]
    use zingo_status::confirmation_status::ConfirmationStatus;

    #[cfg(test)]
    use crate::config::ZingoConfigBuilder;
    #[cfg(test)]
    use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;
    #[cfg(test)]
    use crate::mocks::SaplingCryptoNoteBuilder;
    #[cfg(test)]
    use crate::wallet::notes::orchard::mocks::OrchardNoteBuilder;
    #[cfg(test)]
    use crate::wallet::notes::sapling::mocks::SaplingNoteBuilder;
    #[cfg(test)]
    use crate::wallet::notes::transparent::mocks::TransparentOutputBuilder;
    #[cfg(test)]
    use crate::wallet::transaction_record::mocks::TransactionRecordBuilder;
    #[cfg(test)]
    use crate::wallet::WalletBase;

    #[tokio::test]
    async fn confirmed_balance_excluding_dust() {
        let wallet = LightWallet::new(
            ZingoConfigBuilder::default().create().chain,
            WalletBase::FreshEntropy,
            1.into(),
        )
        .unwrap();
        let confirmed_tx_record = TransactionRecordBuilder::default()
            .status(ConfirmationStatus::Confirmed(80.into()))
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default())
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .note(
                        SaplingCryptoNoteBuilder::default()
                            .value(sapling_crypto::value::NoteValue::from_raw(3_000))
                            .clone(),
                    )
                    .clone(),
            )
            .orchard_notes(OrchardNoteBuilder::default())
            .orchard_notes(OrchardNoteBuilder::default())
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .note(
                        OrchardCryptoNoteBuilder::default()
                            .value(orchard::value::NoteValue::from_raw(5_001))
                            .clone(),
                    )
                    .clone(),
            )
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .note(
                        OrchardCryptoNoteBuilder::default()
                            .value(orchard::value::NoteValue::from_raw(2_000))
                            .clone(),
                    )
                    .clone(),
            )
            .build();
        let mempool_tx_record = TransactionRecordBuilder::default()
            .status(ConfirmationStatus::Mempool(95.into()))
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default())
            .orchard_notes(OrchardNoteBuilder::default())
            .build();
        {
            let mut tx_map = wallet
                .transaction_context
                .transaction_metadata_set
                .write()
                .await;
            tx_map
                .transaction_records_by_id
                .insert_transaction_record(confirmed_tx_record);
            tx_map
                .transaction_records_by_id
                .insert_transaction_record(mempool_tx_record);
        }

        assert_eq!(
            wallet.confirmed_balance_excluding_dust::<Sapling>().await,
            Some(400_000)
        );
        assert_eq!(
            wallet.confirmed_balance_excluding_dust::<Orchard>().await,
            Some(1_605_001)
        );
    }
}

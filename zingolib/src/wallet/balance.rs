//! Balance methods and types for `crate::wallet::LightWallet`.

use pepper_sync::wallet::{
    KeyIdInterface, OrchardNote, OutputInterface, SaplingNote, TransparentCoin, WalletTransaction,
};
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::{PoolType, value::Zatoshis};

use crate::utils;

use super::{
    LightWallet,
    error::{BalanceError, KeyError},
    keys::unified::UnifiedKeyStore,
};

/// Balance for a wallet account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountBalance {
    /// Sum of unspent orchard note values in confirmed blocks excluding dust.
    pub confirmed_orchard_balance: Option<Zatoshis>,
    /// Sum of unspent orchard note values in unconfirmed blocks excluding dust.
    pub unconfirmed_orchard_balance: Option<Zatoshis>,
    /// Sum of confirmed and unconfirmed orchard balances.
    pub total_orchard_balance: Option<Zatoshis>,

    /// Sum of unspent sapling note values in confirmed blocks excluding dust.
    pub confirmed_sapling_balance: Option<Zatoshis>,
    /// Sum of unspent sapling note values in unconfirmed blocks excluding dust.
    pub unconfirmed_sapling_balance: Option<Zatoshis>,
    /// Sum of confirmed and unconfirmed sapling balances.
    pub total_sapling_balance: Option<Zatoshis>,

    /// Sum of unspent transparent coin values in confirmed blocks excluding dust.
    pub confirmed_transparent_balance: Option<Zatoshis>,
    /// Sum of unspent transparent coin values in unconfirmed blocks excluding dust.
    pub unconfirmed_transparent_balance: Option<Zatoshis>,
    /// Sum of confirmed and unconfirmed transparent balances.
    pub total_transparent_balance: Option<Zatoshis>,
}

impl std::fmt::Display for AccountBalance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[
    confirmed_orchard_balance: {}
    unconfirmed_orchard_balance: {}
    total_orchard_balance: {}

    confirmed_sapling_balance: {}
    unconfirmed_sapling_balance: {}
    total_sapling_balance: {}

    confirmed_transparent_balance: {}
    unconfirmed_transparent_balance: {}
    total_transparent_balance: {}
]",
            self.confirmed_orchard_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
            self.unconfirmed_orchard_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
            self.total_orchard_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
            self.confirmed_sapling_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
            self.unconfirmed_sapling_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
            self.total_sapling_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
            self.confirmed_transparent_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
            self.unconfirmed_transparent_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
            self.total_transparent_balance
                .map_or("no view capability".to_string(), |zats| {
                    format_zatoshis(zats)
                }),
        )
    }
}

impl From<AccountBalance> for json::JsonValue {
    fn from(value: AccountBalance) -> Self {
        json::object! {
            "confirmed_orchard_balance" => value.confirmed_orchard_balance.map(|bal| bal.into_u64()),
            "unconfirmed_orchard_balance" => value.unconfirmed_orchard_balance.map(|bal| bal.into_u64()),
            "total_orchard_balance" => value.total_orchard_balance.map(|bal| bal.into_u64()),
            "confirmed_sapling_balance" => value.confirmed_sapling_balance.map(|bal| bal.into_u64()),
            "unconfirmed_sapling_balance" => value.unconfirmed_sapling_balance.map(|bal| bal.into_u64()),
            "total_sapling_balance" => value.total_sapling_balance.map(|bal| bal.into_u64()),
            "confirmed_transparent_balance" => value.confirmed_transparent_balance.map(|bal| bal.into_u64()),
            "unconfirmed_transparent_balance" => value.unconfirmed_transparent_balance.map(|bal| bal.into_u64()),
            "total_transparent_balance" => value.total_transparent_balance.map(|bal| bal.into_u64()),
        }
    }
}

fn format_zatoshis(zatoshis: Zatoshis) -> String {
    let zats = zatoshis.into_u64().to_string();
    let digits: Vec<char> = zats.chars().rev().collect();
    let mut formatted = String::new();

    for (index, digit) in digits.iter().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted.push('_');
        }
        formatted.push(*digit);
    }

    formatted.chars().rev().collect()
}

impl LightWallet {
    /// Returns account balance.
    pub async fn account_balance(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<AccountBalance, BalanceError> {
        let confirmed_orchard_balance =
            match self.confirmed_balance::<OrchardNote>(account_id).await {
                Ok(zats) => Some(zats),
                Err(BalanceError::KeyError(KeyError::NoViewCapability)) => None,
                Err(e) => return Err(e),
            };
        let unconfirmed_orchard_balance =
            match self.unconfirmed_balance::<OrchardNote>(account_id).await {
                Ok(zats) => Some(zats),
                Err(BalanceError::KeyError(KeyError::NoViewCapability)) => None,
                Err(e) => return Err(e),
            };
        let total_orchard_balance =
            confirmed_orchard_balance.and_then(|confirmed| unconfirmed_orchard_balance + confirmed);

        let confirmed_sapling_balance =
            match self.confirmed_balance::<SaplingNote>(account_id).await {
                Ok(zats) => Some(zats),
                Err(BalanceError::KeyError(KeyError::NoViewCapability)) => None,
                Err(e) => return Err(e),
            };
        let unconfirmed_sapling_balance =
            match self.unconfirmed_balance::<SaplingNote>(account_id).await {
                Ok(zats) => Some(zats),
                Err(BalanceError::KeyError(KeyError::NoViewCapability)) => None,
                Err(e) => return Err(e),
            };
        let total_sapling_balance =
            confirmed_sapling_balance.and_then(|confirmed| unconfirmed_sapling_balance + confirmed);

        let confirmed_transparent_balance =
            match self.confirmed_balance::<TransparentCoin>(account_id).await {
                Ok(zats) => Some(zats),
                Err(BalanceError::KeyError(KeyError::NoViewCapability)) => None,
                Err(e) => return Err(e),
            };
        let unconfirmed_transparent_balance = match self
            .unconfirmed_balance::<TransparentCoin>(account_id)
            .await
        {
            Ok(zats) => Some(zats),
            Err(BalanceError::KeyError(KeyError::NoViewCapability)) => None,
            Err(e) => return Err(e),
        };
        let total_transparent_balance = confirmed_transparent_balance
            .and_then(|confirmed| unconfirmed_transparent_balance + confirmed);

        Ok(AccountBalance {
            confirmed_orchard_balance,
            unconfirmed_orchard_balance,
            total_orchard_balance,
            confirmed_sapling_balance,
            unconfirmed_sapling_balance,
            total_sapling_balance,
            confirmed_transparent_balance,
            unconfirmed_transparent_balance,
            total_transparent_balance,
        })
    }

    /// Returns the total balance of the all outputs of a given pool type and `account_id` matching the output and transaction
    /// criteria specified by the `filter_function`.
    ///
    /// # Error
    ///
    /// Returns an error if:
    /// - no keys are found for the given `account_id`
    /// - the UFVK does not have viewing capability for the given pool type
    /// - the balance summation exceeds the valid range of zatoshis
    pub async fn get_filtered_balance<Op, F>(
        &self,
        filter_function: F,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, BalanceError>
    where
        Op: OutputInterface,
        F: Fn(&Op, &WalletTransaction) -> bool,
    {
        match &self
            .unified_key_store
            .get(&account_id)
            .ok_or(KeyError::NoAccountKeys)?
        {
            UnifiedKeyStore::Spend(_) => (),
            UnifiedKeyStore::View(ufvk) => match Op::POOL_TYPE {
                PoolType::Transparent => {
                    if ufvk.transparent().is_none() {
                        return Err(KeyError::NoViewCapability.into());
                    };
                }
                PoolType::SAPLING => {
                    if ufvk.sapling().is_none() {
                        return Err(KeyError::NoViewCapability.into());
                    };
                }
                PoolType::ORCHARD => {
                    if ufvk.orchard().is_none() {
                        return Err(KeyError::NoViewCapability.into());
                    };
                }
            },
            UnifiedKeyStore::Empty => return Err(KeyError::NoViewCapability.into()),
        }

        Ok(utils::conversion::zatoshis_from_u64(
            self.wallet_transactions
                .values()
                .fold(0, |acc, transaction| {
                    acc + Op::transaction_outputs(transaction)
                        .iter()
                        .filter(|&output| {
                            filter_function(output, transaction)
                                && output.spending_transaction().is_none()
                                && output.key_id().account_id() == account_id
                        })
                        .map(|output| output.value())
                        .sum::<u64>()
                }),
        )?)
    }

    /// Returns total wallet balance of unspent notes in confirmed blocks for a given shielded pool and `account_id`.
    ///
    /// # Error
    ///
    /// Returns an error if:
    /// - no keys are found for the given `account_id`
    /// - the UFVK does not have viewing capability for the given pool type
    /// - the balance summation exceeds the valid range of zatoshis
    pub async fn confirmed_balance<Op>(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, BalanceError>
    where
        Op: OutputInterface,
    {
        self.get_filtered_balance::<Op, _>(
            |_, transaction: &WalletTransaction| transaction.status().is_confirmed(),
            account_id,
        )
        .await
    }

    /// Returns total wallet balance of unspent notes not yet confirmed on the block chain for a given shielded pool
    /// and `account_id`.
    ///
    /// # Error
    ///
    /// Returns an error if:
    /// - no keys are found for the given `account_id`
    /// - the UFVK does not have viewing capability for the given pool type
    /// - the balance summation exceeds the valid range of zatoshis
    pub async fn unconfirmed_balance<Op>(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, BalanceError>
    where
        Op: OutputInterface,
    {
        self.get_filtered_balance::<Op, _>(
            |_, transaction: &WalletTransaction| !transaction.status().is_confirmed(),
            account_id,
        )
        .await
    }

    /// Returns total wallet balance of unspent notes in confirmed blocks for a given shielded pool excluding any notes
    /// with value less than marginal fee (5_000).
    ///
    /// # Error
    ///
    /// Returns an error if:
    /// - no keys are found for the given `account_id`
    /// - the UFVK does not have viewing capability for the given pool type
    /// - the balance summation exceeds the valid range of zatoshis
    pub async fn confirmed_balance_excluding_dust<Op>(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, BalanceError>
    where
        Op: OutputInterface,
    {
        self.get_filtered_balance::<Op, _>(
            |note, transaction: &WalletTransaction| {
                Op::value(note) > MARGINAL_FEE.into_u64() && transaction.status().is_confirmed()
            },
            account_id,
        )
        .await
    }

    /// Returns total balance of all shielded pools excluding any notes with value less than marginal fee
    /// that are confirmed on the block chain (the block has at least 1 confirmation).
    /// Does not include transparent funds.
    ///
    /// # Error
    ///
    /// Returns an error if:
    /// - no keys are found for the given `account_id`
    /// - the balance summation exceeds the valid range of zatoshis
    pub async fn confirmed_shielded_balance_excluding_dust(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<Zatoshis, BalanceError> {
        let orchard_balance = match self
            .confirmed_balance_excluding_dust::<OrchardNote>(account_id)
            .await
        {
            Ok(zats) => Ok(zats),
            Err(BalanceError::KeyError(KeyError::NoViewCapability)) => Ok(Zatoshis::ZERO),
            Err(e) => Err(e),
        }?;
        let sapling_balance = match self
            .confirmed_balance_excluding_dust::<SaplingNote>(account_id)
            .await
        {
            Ok(zats) => Ok(zats),
            Err(BalanceError::KeyError(KeyError::NoViewCapability)) => Ok(Zatoshis::ZERO),
            Err(e) => Err(e),
        }?;

        (orchard_balance + sapling_balance).ok_or(BalanceError::Overflow)
    }
}

#[cfg(any(test, feature = "test-elevation"))]
mod test {
    // FIXME: zingo2 rewrite as an integration test
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

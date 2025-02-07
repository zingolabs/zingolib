// FIXME: zingo2
#![allow(unused_variables)]

use std::{collections::HashMap, num::NonZeroU32, ops::Range};

use zcash_client_backend::{
    data_api::{
        scanning::ScanRange, Account, BlockMetadata, InputSource, NullifierQuery, SpendableNotes,
        TransactionDataRequest, WalletRead, WalletSummary,
    },
    wallet::{NoteId, ReceivedNote, TransparentAddressMetadata, WalletTransparentOutput},
    ShieldedProtocol,
};
use zcash_keys::{address::UnifiedAddress, keys::UnifiedFullViewingKey};
use zcash_primitives::{
    block::BlockHash,
    consensus::BlockHeight,
    legacy::TransparentAddress,
    memo::Memo,
    transaction::{
        components::{amount::NonNegativeAmount, OutPoint},
        fees::zip317::MARGINAL_FEE,
        Transaction, TxId,
    },
};

use super::{
    error::{InputSourceError, WalletError},
    LightWallet,
};

enum RemainingNeeded {
    Positive(NonNegativeAmount),
    GracelessChangeAmount(NonNegativeAmount),
}

pub struct ZingoAccount(zip32::AccountId, UnifiedFullViewingKey);

impl Account for ZingoAccount {
    type AccountId = zip32::AccountId;

    fn id(&self) -> Self::AccountId {
        self.0
    }

    fn source(&self) -> zcash_client_backend::data_api::AccountSource {
        unimplemented!()
    }

    fn ufvk(&self) -> Option<&UnifiedFullViewingKey> {
        Some(&self.1)
    }

    fn uivk(&self) -> zcash_keys::keys::UnifiedIncomingViewingKey {
        unimplemented!()
    }
}

impl WalletRead for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;
    type Account = ZingoAccount;

    fn get_account_ids(&self) -> Result<Vec<Self::AccountId>, Self::Error> {
        unimplemented!()
    }

    fn get_account(
        &self,
        account_id: Self::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn get_derived_account(
        &self,
        seed: &zip32::fingerprint::SeedFingerprint,
        account_id: zip32::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn validate_seed(
        &self,
        account_id: Self::AccountId,
        seed: &secrecy::SecretVec<u8>,
    ) -> Result<bool, Self::Error> {
        unimplemented!()
    }

    fn seed_relevance_to_derived_accounts(
        &self,
        seed: &secrecy::SecretVec<u8>,
    ) -> Result<zcash_client_backend::data_api::SeedRelevance<Self::AccountId>, Self::Error> {
        unimplemented!()
    }

    fn get_account_for_ufvk(
        &self,
        ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn get_current_address(
        &self,
        account: Self::AccountId,
    ) -> Result<Option<UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn get_account_birthday(&self, account: Self::AccountId) -> Result<BlockHeight, Self::Error> {
        unimplemented!()
    }

    fn get_wallet_birthday(&self) -> Result<Option<BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_wallet_summary(
        &self,
        min_confirmations: u32,
    ) -> Result<Option<WalletSummary<Self::AccountId>>, Self::Error> {
        unimplemented!()
    }

    fn chain_height(&self) -> Result<Option<BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_block_hash(&self, block_height: BlockHeight) -> Result<Option<BlockHash>, Self::Error> {
        unimplemented!()
    }

    fn block_metadata(&self, height: BlockHeight) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn block_fully_scanned(&self) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn get_max_height_hash(&self) -> Result<Option<(BlockHeight, BlockHash)>, Self::Error> {
        unimplemented!()
    }

    fn block_max_scanned(&self) -> Result<Option<BlockMetadata>, Self::Error> {
        unimplemented!()
    }

    fn suggest_scan_ranges(&self) -> Result<Vec<ScanRange>, Self::Error> {
        unimplemented!()
    }

    fn get_target_and_anchor_heights(
        &self,
        min_confirmations: NonZeroU32,
    ) -> Result<Option<(BlockHeight, BlockHeight)>, Self::Error> {
        unimplemented!()
    }

    fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<Self::AccountId, UnifiedFullViewingKey>, Self::Error> {
        unimplemented!()
    }

    fn get_memo(&self, note_id: NoteId) -> Result<Option<Memo>, Self::Error> {
        unimplemented!()
    }

    fn get_transaction(&self, txid: TxId) -> Result<Option<Transaction>, Self::Error> {
        unimplemented!()
    }

    fn get_sapling_nullifiers(
        &self,
        query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, sapling_crypto::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_orchard_nullifiers(
        &self,
        query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_receivers(
        &self,
        _account: Self::AccountId,
    ) -> Result<HashMap<TransparentAddress, Option<TransparentAddressMetadata>>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_balances(
        &self,
        _account: Self::AccountId,
        _max_height: BlockHeight,
    ) -> Result<HashMap<TransparentAddress, NonNegativeAmount>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_address_metadata(
        &self,
        account: Self::AccountId,
        address: &TransparentAddress,
    ) -> Result<Option<TransparentAddressMetadata>, Self::Error> {
        unimplemented!()
    }

    fn get_known_ephemeral_addresses(
        &self,
        _account: Self::AccountId,
        _index_range: Option<Range<u32>>,
    ) -> Result<Vec<(TransparentAddress, TransparentAddressMetadata)>, Self::Error> {
        unimplemented!()
    }

    fn transaction_data_requests(&self) -> Result<Vec<TransactionDataRequest>, Self::Error> {
        unimplemented!()
    }
}

impl InputSource for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;
    type NoteRef = NoteId;

    fn get_spendable_note(
        &self,
        txid: &TxId,
        protocol: ShieldedProtocol,
        index: u32,
    ) -> Result<
        Option<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        unimplemented!()
    }

    // TODO: rework to operate on notes themselves so its not needed to find them in the wallet and more info is
    // available for advanced selection
    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: NonNegativeAmount,
        sources: &[ShieldedProtocol],
        anchor_height: BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<Self::NoteRef>, Self::Error> {
        let mut unselected =
            self.get_spendable_note_ids_and_values(sources, anchor_height, exclude);

        unselected.sort_by_key(|(_id, value)| *value); // from smallest to largest
        let dust_spendable_index =
            unselected.partition_point(|(_id, value)| *value <= MARGINAL_FEE);
        let _dust_notes: Vec<_> = unselected.drain(..dust_spendable_index).collect();
        let mut selected = vec![];
        let mut index_of_unselected = 0;

        loop {
            // if no unselected notes are available, return the currently selected notes even if the target value has not been reached
            if unselected.is_empty() {
                break;
            }
            // update target value for further note selection
            let selected_notes_total_value = selected
                .iter()
                .try_fold(NonNegativeAmount::ZERO, |acc, (_id, value)| acc + *value)
                .ok_or(InputSourceError::InvalidValue(
                    zcash_primitives::transaction::components::amount::BalanceError::Overflow,
                ))?;
            let updated_target_value =
                match calculate_remaining_needed(target_value, selected_notes_total_value) {
                    RemainingNeeded::Positive(updated_target_value) => updated_target_value,
                    RemainingNeeded::GracelessChangeAmount(_change) => {
                        break;
                    }
                };

            match unselected.get(index_of_unselected) {
                Some(smallest_unselected) => {
                    // selected a note to test if it has enough value to complete the transaction on its own
                    if smallest_unselected.1 >= updated_target_value {
                        selected.push(*smallest_unselected);
                        unselected.remove(index_of_unselected);
                    } else {
                        // this note is not big enough. try the next
                        index_of_unselected += 1;
                    }
                }
                None => {
                    // the iterator went off the end of the vector without finding a note big enough to complete the transaction
                    // add the biggest note and reset the iteration
                    selected.push(unselected.pop().expect("should be nonempty")); // TODO:  Add soundness proving unit-test
                    index_of_unselected = 0;
                }
            }
        }

        /* TODO: Priority
        if selected
            .iter()
            .filter(|n| n.0.protocol() == ShieldedProtocol::Sapling)
            .count()
            == 1
            || selected
                .iter()
                .filter(|n| n.0.protocol() == ShieldedProtocol::Orchard)
                .count()
                == 1
        {
            // since we maxed out the target value with only one note in at least one Shielded Pool
            //  we have an option to sweep a dust note into a grace input.
            // we will sweep the biggest dust note we can
            if !dust_notes.is_empty() {
                sweep_dust_into_grace(&mut selected, dust_notes);
            }
            // TODO: re-introduce this optimisation, current bug is that we don't select a note from the same pool as the single selected note
            // (and we don't have information about the pool(s) the outputs are being created for)
            // this is ok for dust as it is excluded if the dust is from a pool where grace inputs are available. however, this doesn't work for
            // non-dust
            //
            // } else {
            //     // we have no extra dust, but we can still save a marginal fee by adding the next smallest note to change
            //     if let Some(smallest_note) = unselected.pop() {
            //         selected.push(smallest_note);
            //     };
            // }
        }
        */

        let mut selected_sapling = Vec::<ReceivedNote<NoteId, sapling_crypto::Note>>::new();
        let mut selected_orchard = Vec::<ReceivedNote<NoteId, orchard::Note>>::new();

        // transform each NoteId to a ReceivedNote
        selected.iter().try_for_each(|(id, _value)| {
            let transaction = self
                .wallet_transactions
                .get(id.txid())
                .expect("should exist as note_id is created from the record itself");
            let output_index = id.output_index() as u32;
            match id.protocol() {
                ShieldedProtocol::Sapling => transaction
                    .get_received_note::<SaplingDomain>(output_index)
                    .map(|received_note| {
                        selected_sapling.push(received_note);
                    }),
                ShieldedProtocol::Orchard => transaction
                    .get_received_note::<OrchardDomain>(output_index)
                    .map(|received_note| {
                        selected_orchard.push(received_note);
                    }),
            }
            .ok_or(InputSourceError::WitnessPositionNotFound(*id))
        })?;

        Ok(SpendableNotes::new(selected_sapling, selected_orchard))
    }

    fn get_unspent_transparent_output(
        &self,
        _outpoint: &OutPoint,
    ) -> Result<Option<WalletTransparentOutput>, Self::Error> {
        unimplemented!()
    }

    fn get_spendable_transparent_outputs(
        &self,
        _address: &TransparentAddress,
        _target_height: BlockHeight,
        _min_confirmations: u32,
    ) -> Result<Vec<WalletTransparentOutput>, Self::Error> {
        unimplemented!()
    }
}

/// Calculate remaining difference between target and selected.
/// There are two mutually exclusive cases:
///    (Change) There's no more needed so we've selected 0 or more change
///    (Positive) We need > 0 more value.
/// This function represents the NonPositive case as None, which then serves to signal a break in the note selection
/// for where this helper is uniquely called.
fn calculate_remaining_needed(
    target_value: NonNegativeAmount,
    selected_value: NonNegativeAmount,
) -> RemainingNeeded {
    if let Some(amount) = target_value - selected_value {
        if amount == NonNegativeAmount::ZERO {
            // Case (Change) target_value == total_selected_value
            RemainingNeeded::GracelessChangeAmount(NonNegativeAmount::ZERO)
        } else {
            // Case (Positive) target_value > total_selected_value
            RemainingNeeded::Positive(amount)
        }
    } else {
        // Case (Change) target_value < total_selected_value
        // Return the non-zero change quantity
        RemainingNeeded::GracelessChangeAmount(
            (selected_value - target_value).expect("This is guaranteed positive"),
        )
    }
}

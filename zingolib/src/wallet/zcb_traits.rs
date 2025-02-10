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
        Transaction, TxId,
    },
};
use zingo_sync::{
    keys::transparent,
    primitives::{NoteInterface, OutputId, OutputInterface},
};

use crate::{wallet::notes::RemainingNeeded, Orchard, Sapling};

use super::{error::WalletError, LightWallet};

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

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: NonNegativeAmount,
        sources: &[ShieldedProtocol],
        anchor_height: BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<Self::NoteRef>, Self::Error> {
        let exclude_sapling = exclude
            .iter()
            .filter(|&note_id| note_id.protocol() == ShieldedProtocol::Sapling)
            .map(|note_id| OutputId::from_parts(*note_id.txid(), note_id.output_index() as usize))
            .collect::<Vec<_>>();
        let exclude_orchard = exclude
            .iter()
            .filter(|&note_id| note_id.protocol() == ShieldedProtocol::Orchard)
            .map(|note_id| OutputId::from_parts(*note_id.txid(), note_id.output_index() as usize))
            .collect::<Vec<_>>();
        let mut remaining_value_needed = RemainingNeeded::Positive(target_value);

        let selected_sapling_notes = if sources.contains(&ShieldedProtocol::Sapling) {
            self.select_spendable_notes_by_pool::<Sapling>(
                &mut remaining_value_needed,
                anchor_height,
                &exclude_sapling,
            )?
        } else {
            Vec::new()
        };
        let selected_orchard_notes = if sources.contains(&ShieldedProtocol::Orchard) {
            self.select_spendable_notes_by_pool::<Orchard>(
                &mut remaining_value_needed,
                anchor_height,
                &exclude_orchard,
            )?
        } else {
            Vec::new()
        };

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

        let sapling_recieved_notes = selected_sapling_notes
            .iter()
            .map(|note| {
                ReceivedNote::from_parts(
                    NoteId::new(
                        note.output_id().txid(),
                        ShieldedProtocol::Sapling,
                        note.output_id().output_index() as u16,
                    ),
                    note.output_id().txid(),
                    note.output_id().output_index() as u16,
                    note.note().clone(),
                    note.key_id().scope,
                    note.position()
                        .expect("note selection should filter on notes with positions"),
                )
            })
            .collect::<Vec<_>>();
        let orchard_recieved_notes = selected_orchard_notes
            .iter()
            .map(|note| {
                ReceivedNote::from_parts(
                    NoteId::new(
                        note.output_id().txid(),
                        ShieldedProtocol::Orchard,
                        note.output_id().output_index() as u16,
                    ),
                    note.output_id().txid(),
                    note.output_id().output_index() as u16,
                    note.note().clone(),
                    note.key_id().scope,
                    note.position()
                        .expect("note selection should filter on notes with positions"),
                )
            })
            .collect::<Vec<_>>();

        Ok(SpendableNotes::new(
            sapling_recieved_notes,
            orchard_recieved_notes,
        ))
    }

    fn get_unspent_transparent_output(
        &self,
        _outpoint: &OutPoint,
    ) -> Result<Option<WalletTransparentOutput>, Self::Error> {
        unimplemented!()
    }

    fn get_spendable_transparent_outputs(
        &self,
        address: &TransparentAddress,
        target_height: BlockHeight,
        min_confirmations: u32,
    ) -> Result<Vec<WalletTransparentOutput>, Self::Error> {
        let address = transparent::encode_address(&self.network, *address);

        Ok(self
            .spendable_transparent_coins(target_height, &[], min_confirmations)
            .into_iter()
            .filter(|&output| output.address == address)
            .flat_map(|output| {
                WalletTransparentOutput::from_parts(
                    output.output_id().into(),
                    zcash_primitives::transaction::components::TxOut {
                        value: output.value,
                        script_pubkey: output.script.clone(),
                    },
                    Some(
                        self.output_transaction(output)
                            .status()
                            .get_confirmed_height()
                            .expect("output must be confirmed in this scope"),
                    ),
                )
            })
            .collect())
    }
}

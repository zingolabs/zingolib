use std::{collections::HashMap, convert::Infallible, num::NonZeroU32};

use secrecy::SecretVec;
use shardtree::{ShardTree, error::ShardTreeError, store::ShardStore};
use zcash_address::ZcashAddress;
use zcash_client_backend::{
    data_api::{
        Account, AccountBirthday, AccountPurpose, Balance, BlockMetadata, CoinbaseFilter,
        InputSource, NullifierQuery, ORCHARD_SHARD_HEIGHT, ReceivedNotes,
        ReceivedTransactionOutput, SAPLING_SHARD_HEIGHT, TargetValue, TransactionDataRequest,
        TransparentKeyOrigin, WalletCommitmentTrees, WalletRead, WalletSummary, WalletWrite,
        Zip32Derivation,
        chain::{ChainState, CommitmentTreeRoot},
        error::FindAccountForAddressError,
        wallet::{ConfirmationsPolicy, TargetHeight},
    },
    wallet::{Exposure, NoteId, ReceivedNote, TransparentAddressMetadata, WalletTransparentOutput},
};
use zcash_keys::{address::UnifiedAddress, keys::UnifiedFullViewingKey};
use zcash_primitives::{
    block::BlockHash,
    transaction::{Transaction, TxId},
};
use zcash_protocol::{
    PoolType, ShieldedPool,
    consensus::{self, BlockHeight, Parameters},
    memo::Memo,
};
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::bundle::{OutPoint, TxOut};
use zcash_transparent::keys::TransparentKeyScope;

use super::{LightWallet, error::WalletError, output::OutputRef};
use crate::wallet::output::RemainingNeeded;
use pepper_sync::{
    error::SyncError,
    keys::transparent::{self, TransparentScope},
    wallet::{
        IronwoodNote, KeyIdInterface, NoteInterface, OrchardNote, OrchardShardStore, OutputId,
        OutputInterface, SaplingNote, SaplingShardStore, traits::SyncWallet,
    },
};
use zingo_status::confirmation_status::ConfirmationStatus;

pub struct ZingoAccount(zip32::AccountId, UnifiedFullViewingKey);

impl Account for ZingoAccount {
    type AccountId = zip32::AccountId;

    fn id(&self) -> Self::AccountId {
        self.0
    }

    fn name(&self) -> Option<&str> {
        None
    }

    fn birthday_height(&self) -> BlockHeight {
        unimplemented!()
    }

    fn source(&self) -> &zcash_client_backend::data_api::AccountSource {
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
        Ok(self.unified_key_store.keys().copied().collect())
    }

    fn get_account(
        &self,
        _account_id: Self::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn get_derived_account(
        &self,
        _account_id: &Zip32Derivation,
    ) -> Result<Option<Self::Account>, Self::Error> {
        unimplemented!()
    }

    fn validate_seed(
        &self,
        _account_id: Self::AccountId,
        _seed: &secrecy::SecretVec<u8>,
    ) -> Result<bool, Self::Error> {
        unimplemented!()
    }

    fn seed_relevance_to_derived_accounts(
        &self,
        _seed: &secrecy::SecretVec<u8>,
    ) -> Result<zcash_client_backend::data_api::SeedRelevance<Self::AccountId>, Self::Error> {
        unimplemented!()
    }

    fn get_account_for_ufvk(
        &self,
        ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        let Some((account_id, unified_key)) =
            self.unified_key_store.iter().find(|(_, unified_key)| {
                UnifiedFullViewingKey::try_from(*unified_key).is_ok_and(|account_ufvk| {
                    account_ufvk.encode(&self.chain_type) == *ufvk.encode(&self.chain_type)
                })
            })
        else {
            return Ok(None);
        };

        Ok(Some(ZingoAccount(*account_id, unified_key.try_into()?)))
    }

    fn list_addresses(
        &self,
        _account: Self::AccountId,
    ) -> Result<Vec<zcash_client_backend::data_api::AddressInfo>, Self::Error> {
        unimplemented!()
    }

    fn get_last_generated_address_matching(
        &self,
        _account: Self::AccountId,
        _address_filter: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn get_account_birthday(&self, _account: Self::AccountId) -> Result<BlockHeight, Self::Error> {
        unimplemented!()
    }

    fn get_wallet_birthday(&self) -> Result<Option<BlockHeight>, Self::Error> {
        unimplemented!()
    }

    fn get_wallet_summary(
        &self,
        _min_confirmations: ConfirmationsPolicy,
    ) -> Result<Option<WalletSummary<Self::AccountId>>, Self::Error> {
        unimplemented!()
    }

    fn chain_height(&self) -> Result<Option<BlockHeight>, Self::Error> {
        Ok(self.sync_state.last_known_chain_height())
    }

    fn get_block_hash(&self, _block_height: BlockHeight) -> Result<Option<BlockHash>, Self::Error> {
        unimplemented!()
    }

    fn block_metadata(&self, _height: BlockHeight) -> Result<Option<BlockMetadata>, Self::Error> {
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

    fn suggest_scan_ranges(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::scanning::ScanRange>, Self::Error> {
        unimplemented!()
    }

    fn get_target_and_anchor_heights(
        &self,
        min_confirmations: NonZeroU32,
    ) -> Result<Option<(TargetHeight, BlockHeight)>, Self::Error> {
        let target_height = if let Some(height) = self.sync_state.last_known_chain_height() {
            height + 1
        } else {
            return Ok(None);
        };

        let max_checkpoint_height = self
            .shard_trees
            .sapling
            .store()
            .max_checkpoint_id()
            .expect("infallible")
            .expect("should be at least 1 checkpoint");

        let anchor_height = std::cmp::min(
            max_checkpoint_height,
            target_height - min_confirmations.get(),
        );

        Ok(Some((
            target_height.into(),
            std::cmp::max(1.into(), anchor_height),
        )))
    }

    fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        Ok(self
            .wallet_transactions
            .get(&txid)
            .and_then(|transaction| transaction.status().get_confirmed_height()))
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<Self::AccountId, UnifiedFullViewingKey>, Self::Error> {
        unimplemented!()
    }

    fn get_memo(&self, _note_id: NoteId) -> Result<Option<Memo>, Self::Error> {
        unimplemented!()
    }

    fn get_transaction(&self, _txid: TxId) -> Result<Option<Transaction>, Self::Error> {
        unimplemented!()
    }

    fn get_sapling_nullifiers(
        &self,
        _query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, sapling_crypto::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_orchard_nullifiers(
        &self,
        _query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_ironwood_nullifiers(
        &self,
        _query: NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_receivers(
        &self,
        account: Self::AccountId,
        // TODO: only get internal receivers if true
        _include_change: bool,
        _include_standalone_receivers: bool,
    ) -> Result<HashMap<TransparentAddress, TransparentAddressMetadata>, Self::Error> {
        self.transparent_addresses
            .iter()
            .filter(|(address_id, _)| {
                address_id.account_id() == account && address_id.scope() != TransparentScope::Refund
            })
            .map(|(address_id, encoded_address)| {
                let address = ZcashAddress::try_from_encoded(encoded_address)?
                    .convert_if_network::<TransparentAddress>(self.chain_type.network_type())
                    .expect("incorrect network should be checked on wallet load");
                let address_metadata = TransparentAddressMetadata::derived(
                    address_id.scope().into(),
                    address_id.address_index(),
                    Exposure::CannotKnow, // TODO: add exposure to wallet transparent address metadata
                    None,
                );

                Ok((address, address_metadata))
            })
            .collect()
    }

    fn get_ephemeral_transparent_receivers(
        &self,
        account: Self::AccountId,
        _exposure_depth: u32,
        _exclude_used: bool,
    ) -> Result<HashMap<TransparentAddress, TransparentAddressMetadata>, Self::Error> {
        self.transparent_addresses
            .iter()
            .filter(|(address_id, _)| {
                address_id.account_id() == account && address_id.scope() == TransparentScope::Refund
            })
            .map(|(address_id, encoded_address)| {
                let address = ZcashAddress::try_from_encoded(encoded_address)?
                    .convert_if_network::<TransparentAddress>(self.chain_type.network_type())
                    .expect("incorrect network should be checked on wallet load");
                let address_metadata = TransparentAddressMetadata::derived(
                    address_id.scope().into(),
                    address_id.address_index(),
                    Exposure::CannotKnow, // TODO: add exposure to wallet transparent address metadata
                    None,
                );

                Ok((address, address_metadata))
            })
            .collect()
    }

    fn get_transparent_balances(
        &self,
        _account: Self::AccountId,
        _max_height: TargetHeight,
        _confirmations_policy: ConfirmationsPolicy,
    ) -> Result<HashMap<TransparentAddress, (TransparentKeyOrigin, Balance)>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_address_metadata(
        &self,
        account: Self::AccountId,
        address: &TransparentAddress,
    ) -> Result<Option<TransparentAddressMetadata>, Self::Error> {
        Ok(
            if let Some(result) = self
                .get_transparent_receivers(account, true, true)?
                .get(address)
            {
                Some(result.clone())
            } else {
                self.get_ephemeral_transparent_receivers(account, u32::MAX, false)?
                    .get(address)
                    .cloned()
            },
        )
    }

    fn utxo_query_height(
        &self,
        _account: Self::AccountId,
    ) -> Result<zcash_protocol::consensus::BlockHeight, Self::Error> {
        unimplemented!()
    }

    fn transaction_data_requests(&self) -> Result<Vec<TransactionDataRequest>, Self::Error> {
        unimplemented!()
    }

    fn find_account_for_address<P: consensus::Parameters>(
        &self,
        _params: &P,
        _address: &zcash_keys::address::Address,
    ) -> Result<Option<Self::AccountId>, FindAccountForAddressError<Self::Error>> {
        unimplemented!()
    }

    fn get_received_outputs(
        &self,
        _txid: TxId,
        _target_height: TargetHeight,
        _confirmations_policy: ConfirmationsPolicy,
    ) -> Result<Vec<ReceivedTransactionOutput>, Self::Error> {
        unimplemented!()
    }
}

impl WalletWrite for LightWallet {
    type UtxoRef = u32;

    fn create_account(
        &mut self,
        _account_name: &str,
        _seed: &SecretVec<u8>,
        _birthday: &AccountBirthday,
        _key_source: Option<&str>,
    ) -> Result<(Self::AccountId, zcash_keys::keys::UnifiedSpendingKey), Self::Error> {
        unimplemented!()
    }

    fn import_account_hd(
        &mut self,
        _account_name: &str,
        _seed: &SecretVec<u8>,
        _account_index: zip32::AccountId,
        _birthday: &AccountBirthday,
        _key_source: Option<&str>,
    ) -> Result<(Self::Account, zcash_keys::keys::UnifiedSpendingKey), Self::Error> {
        unimplemented!()
    }

    fn import_account_ufvk(
        &mut self,
        _account_name: &str,
        _unified_key: &UnifiedFullViewingKey,
        _birthday: &AccountBirthday,
        _purpose: AccountPurpose,
        _key_source: Option<&str>,
    ) -> Result<Self::Account, Self::Error> {
        unimplemented!()
    }

    fn delete_account(&mut self, _account: Self::AccountId) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn get_next_available_address(
        &mut self,
        _account: Self::AccountId,
        _request: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<(UnifiedAddress, zip32::DiversifierIndex)>, Self::Error> {
        unimplemented!()
    }

    fn get_address_for_index(
        &mut self,
        _account: Self::AccountId,
        _diversifier_index: zip32::DiversifierIndex,
        _request: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<UnifiedAddress>, Self::Error> {
        unimplemented!()
    }

    fn update_chain_tip(&mut self, _tip_height: BlockHeight) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn put_blocks(
        &mut self,
        _from_state: &zcash_client_backend::data_api::chain::ChainState,
        _blocks: Vec<zcash_client_backend::data_api::ScannedBlock<Self::AccountId>>,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn put_received_transparent_utxo(
        &mut self,
        _output: &WalletTransparentOutput<Self::AccountId>,
    ) -> Result<Self::UtxoRef, Self::Error> {
        unimplemented!()
    }

    fn store_decrypted_tx(
        &mut self,
        _received_tx: zcash_client_backend::data_api::DecryptedTransaction<
            Transaction,
            Self::AccountId,
        >,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn set_tx_trust(&mut self, _txid: TxId, _trusted: bool) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn store_transactions_to_be_sent(
        &mut self,
        transactions: &[zcash_client_backend::data_api::SentTransaction<Self::AccountId>],
    ) -> Result<(), Self::Error> {
        let chain_type = self.chain_type;

        for sent_transaction in transactions {
            // this is a workaround as Transaction does not implement Clone
            let mut transaction_bytes = vec![];
            sent_transaction
                .tx()
                .write(&mut transaction_bytes)
                .map_err(WalletError::TransactionWrite)?;
            let transaction = Transaction::read(
                transaction_bytes.as_slice(),
                consensus::BranchId::for_height(
                    &self.chain_type,
                    sent_transaction.target_height().into(),
                ),
            )
            .map_err(WalletError::TransactionRead)?;

            match pepper_sync::scan_pending_transaction(
                &chain_type,
                &SyncWallet::get_unified_full_viewing_keys(self)?,
                self,
                transaction,
                ConfirmationStatus::Calculated(sent_transaction.target_height().into()),
                sent_transaction.created().unix_timestamp() as u32,
            ) {
                Ok(()) => (),
                Err(SyncError::ScanError(e)) => return Err(e.into()),
                Err(SyncError::WalletError(e)) => return Err(e),
                Err(_) => {
                    panic!("`scan_pending_transactions` should only return scan or wallet errors")
                }
            }
        }

        Ok(())
    }

    fn truncate_to_height(&mut self, _max_height: BlockHeight) -> Result<BlockHeight, Self::Error> {
        unimplemented!()
    }

    fn truncate_to_chain_state(&mut self, _chain_state: ChainState) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn rewind_to_chain_state(
        &mut self,
        _chain_state: ChainState,
        _reset_account_birthdays: std::collections::HashSet<Self::AccountId>,
    ) -> Result<(), zcash_client_backend::data_api::error::RewindError<Self::AccountId, Self::Error>>
    {
        unimplemented!()
    }

    fn reserve_next_n_ephemeral_addresses(
        &mut self,
        account_id: Self::AccountId,
        n: usize,
    ) -> Result<Vec<(TransparentAddress, TransparentAddressMetadata)>, Self::Error> {
        Ok(self
            .generate_refund_addresses(n, account_id)?
            .into_iter()
            .map(|(address_id, address)| {
                (
                    address,
                    TransparentAddressMetadata::derived(
                        TransparentKeyScope::EPHEMERAL,
                        address_id.address_index(),
                        Exposure::CannotKnow, // TODO: add exposure to wallet transparent address metadata
                        None,
                    ),
                )
            })
            .collect())
    }

    fn set_transaction_status(
        &mut self,
        _txid: TxId,
        _status: zcash_client_backend::data_api::TransactionStatus,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn notify_address_checked(
        &mut self,
        _request: zcash_client_backend::data_api::TransactionsInvolvingAddress,
        _as_of_height: BlockHeight,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }
}

impl WalletCommitmentTrees for LightWallet {
    type Error = Infallible;
    type SaplingShardStore<'a> = SaplingShardStore;
    type OrchardShardStore<'a> = OrchardShardStore;

    fn with_sapling_tree_mut<F, A, E>(&mut self, mut callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::SaplingShardStore<'a>,
                { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
                { SAPLING_SHARD_HEIGHT },
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        callback(&mut self.shard_trees.sapling)
    }

    fn put_sapling_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<sapling_crypto::Node>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_sapling_tree_mut(|t| {
            for (root, i) in roots.iter().zip(0u64..) {
                let root_addr = incrementalmerkletree::Address::from_parts(
                    SAPLING_SHARD_HEIGHT.into(),
                    start_index + i,
                );
                t.insert(root_addr, *root.root_hash())?;
            }
            Ok::<_, ShardTreeError<Self::Error>>(())
        })?;

        Ok(())
    }

    fn with_orchard_tree_mut<F, A, E>(&mut self, mut callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::OrchardShardStore<'a>,
                { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                { ORCHARD_SHARD_HEIGHT },
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        callback(&mut self.shard_trees.orchard)
    }

    fn put_orchard_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_orchard_tree_mut(|t| {
            for (root, i) in roots.iter().zip(0u64..) {
                let root_addr = incrementalmerkletree::Address::from_parts(
                    ORCHARD_SHARD_HEIGHT.into(),
                    start_index + i,
                );
                t.insert(root_addr, *root.root_hash())?;
            }
            Ok::<_, ShardTreeError<Self::Error>>(())
        })?;

        Ok(())
    }

    fn with_ironwood_tree_mut<F, A, E>(&mut self, mut callback: F) -> Result<Option<A>, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::OrchardShardStore<'a>,
                { ORCHARD_SHARD_HEIGHT * 2 },
                ORCHARD_SHARD_HEIGHT,
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        callback(&mut self.shard_trees.ironwood).map(Some)
    }
}

/// Ironwood subtree root loading, mirroring `put_orchard_subtree_roots`.
/// The upstream [`WalletCommitmentTrees`] trait has no Ironwood subtree-root
/// method, so this lives here as an inherent method until the trait grows one.
impl LightWallet {
    pub fn with_ironwood_tree_mut_inherent<F, A, E>(&mut self, mut callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                OrchardShardStore,
                { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                { ORCHARD_SHARD_HEIGHT },
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Infallible>>,
    {
        callback(&mut self.shard_trees.ironwood)
    }

    pub fn put_ironwood_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>],
    ) -> Result<(), ShardTreeError<Infallible>> {
        self.with_ironwood_tree_mut_inherent(|t| {
            for (root, i) in roots.iter().zip(0u64..) {
                let root_addr = incrementalmerkletree::Address::from_parts(
                    ORCHARD_SHARD_HEIGHT.into(),
                    start_index + i,
                );
                t.insert(root_addr, *root.root_hash())?;
            }
            Ok::<_, ShardTreeError<Infallible>>(())
        })?;

        Ok(())
    }
}

impl InputSource for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;
    type NoteRef = OutputRef;

    fn get_spendable_note(
        &self,
        _txid: &TxId,
        _protocol: ShieldedPool,
        _index: u32,
        _target_height: TargetHeight,
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
        target_value: TargetValue,
        sources: &[ShieldedPool],
        _target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
        exclude: &[Self::NoteRef],
    ) -> Result<ReceivedNotes<Self::NoteRef>, Self::Error> {
        let (_, anchor_height) = self
            .get_target_and_anchor_heights(confirmations_policy.trusted())
            .expect("infallible")
            .ok_or(WalletError::NoSyncData)?;

        let mut exclude_sapling = exclude
            .iter()
            .filter(|&note_id| note_id.pool_type() == PoolType::SAPLING)
            .map(|note_id| OutputId::new(note_id.txid(), note_id.output_index()))
            .collect::<Vec<_>>();
        let mut exclude_orchard = exclude
            .iter()
            .filter(|&note_id| note_id.pool_type() == PoolType::ORCHARD)
            .map(|note_id| OutputId::new(note_id.txid(), note_id.output_index()))
            .collect::<Vec<_>>();
        let mut exclude_ironwood = exclude
            .iter()
            .filter(|&note_id| note_id.pool_type() == PoolType::IRONWOOD)
            .map(|note_id| OutputId::new(note_id.txid(), note_id.output_index()))
            .collect::<Vec<_>>();

        // Soft reservation: notes bound to pending migration parts are
        // withheld from ordinary selection first, and offered again only if
        // the request cannot be satisfied without them. The reservation
        // biases selection and never blocks a spend.
        let reserved_orchard: Vec<OutputId> = self
            .migration
            .as_ref()
            .map(|migration| {
                migration
                    .reserved_output_ids()
                    .into_iter()
                    .filter(|output_id| !exclude_orchard.contains(output_id))
                    .collect()
            })
            .unwrap_or_default();

        let (selected_sapling_notes, selected_orchard_notes, selected_ironwood_notes) =
            match target_value {
                TargetValue::AtLeast(at_least_value) => {
                    let mut remaining_value_needed = RemainingNeeded::Positive(at_least_value);

                    // prioritises selecting spendable notes that are guaranteed to be unspent first
                    let mut selected_sapling_notes = Vec::new();
                    let mut selected_orchard_notes = Vec::new();
                    let mut selected_ironwood_notes = Vec::new();
                    exclude_orchard.extend(reserved_orchard.iter().copied());
                    for withhold_reserved in [true, false] {
                        if !withhold_reserved {
                            let unmet = matches!(
                                remaining_value_needed,
                                RemainingNeeded::Positive(value) if value.into_u64() > 0
                            );
                            if reserved_orchard.is_empty() || !unmet {
                                break;
                            }
                            exclude_orchard
                                .retain(|output_id| !reserved_orchard.contains(output_id));
                        }
                        for include_potentially_spent_notes in [false, true] {
                            // Prioritise note selection for the given `sources`,
                            // honoring their order: the input selector lists the
                            // caller's preferred pools first (the payment's own
                            // pool leads), and processing them in a fixed order
                            // instead would take inputs from a dispreferred
                            // pool, paying an extra bundle's fee.
                            for source in sources {
                                match source {
                                    ShieldedPool::Sapling => {
                                        let notes = self
                                            .select_spendable_notes_by_pool::<SaplingNote>(
                                                &mut remaining_value_needed,
                                                anchor_height,
                                                &exclude_sapling,
                                                account,
                                                include_potentially_spent_notes,
                                            )?
                                            .into_iter()
                                            .cloned()
                                            .collect::<Vec<_>>();
                                        exclude_sapling
                                            .extend(notes.iter().map(OutputInterface::output_id));
                                        selected_sapling_notes.extend(notes);
                                    }
                                    ShieldedPool::Orchard => {
                                        let notes = self
                                            .select_spendable_notes_by_pool::<OrchardNote>(
                                                &mut remaining_value_needed,
                                                anchor_height,
                                                &exclude_orchard,
                                                account,
                                                include_potentially_spent_notes,
                                            )?
                                            .into_iter()
                                            .cloned()
                                            .collect::<Vec<_>>();
                                        exclude_orchard
                                            .extend(notes.iter().map(OutputInterface::output_id));
                                        selected_orchard_notes.extend(notes);
                                    }
                                    ShieldedPool::Ironwood => {
                                        let notes = self
                                            .select_spendable_notes_by_pool::<IronwoodNote>(
                                                &mut remaining_value_needed,
                                                anchor_height,
                                                &exclude_ironwood,
                                                account,
                                                include_potentially_spent_notes,
                                            )?
                                            .into_iter()
                                            .cloned()
                                            .collect::<Vec<_>>();
                                        exclude_ironwood
                                            .extend(notes.iter().map(OutputInterface::output_id));
                                        selected_ironwood_notes.extend(notes);
                                    }
                                }
                            }

                            let notes = self
                                .select_spendable_notes_by_pool::<SaplingNote>(
                                    &mut remaining_value_needed,
                                    anchor_height,
                                    &exclude_sapling,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            exclude_sapling.extend(notes.iter().map(OutputInterface::output_id));
                            selected_sapling_notes.extend(notes);

                            let notes = self
                                .select_spendable_notes_by_pool::<OrchardNote>(
                                    &mut remaining_value_needed,
                                    anchor_height,
                                    &exclude_orchard,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            exclude_orchard.extend(notes.iter().map(OutputInterface::output_id));
                            selected_orchard_notes.extend(notes);

                            let notes = self
                                .select_spendable_notes_by_pool::<IronwoodNote>(
                                    &mut remaining_value_needed,
                                    anchor_height,
                                    &exclude_ironwood,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            exclude_ironwood.extend(notes.iter().map(OutputInterface::output_id));
                            selected_ironwood_notes.extend(notes);
                        }
                    }
                    (
                        selected_sapling_notes,
                        selected_orchard_notes,
                        selected_ironwood_notes,
                    )
                }
                TargetValue::AllFunds(_max_spend_mode) => {
                    // Upstream drives this arm through propose_send_max; an
                    // unsupported mode must surface through the trait's error
                    // channel, never abort the process.
                    return Err(WalletError::AllFundsSelectionUnsupported);
                }
            };

        /* TODO: Priority
        if selected
            .iter()
            .filter(|n| n.0.protocol() == ShieldedPool::Sapling)
            .count()
            == 1
            || selected
                .iter()
                .filter(|n| n.0.protocol() == ShieldedPool::Orchard)
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
                    OutputRef::new(
                        OutputId::new(note.output_id().txid(), note.output_id().output_index()),
                        PoolType::SAPLING,
                    ),
                    note.output_id().txid(),
                    note.output_id()
                        .output_index()
                        .try_into()
                        .expect("shielded notes are always valid u16"),
                    note.note().clone(),
                    note.key_id().scope,
                    note.position()
                        .expect("note selection should filter on notes with positions"),
                    None, // mined_height. TODO: How should we use this here?
                    None, // max_shielding_input_height. TODO: How should we use this here?
                )
            })
            .collect::<Vec<_>>();
        let orchard_recieved_notes = selected_orchard_notes
            .iter()
            .map(|note| {
                ReceivedNote::from_parts(
                    OutputRef::new(
                        OutputId::new(note.output_id().txid(), note.output_id().output_index()),
                        PoolType::ORCHARD,
                    ),
                    note.output_id().txid(),
                    note.output_id()
                        .output_index()
                        .try_into()
                        .expect("shielded notes are always valid u16"),
                    *note.note(),
                    note.key_id().scope,
                    note.position()
                        .expect("note selection should filter on notes with positions"),
                    None, // mined_height. TODO: How should we use this here?
                    None, // max_shielding_input_height. TODO: How should we use this here?
                )
            })
            .collect::<Vec<_>>();
        let ironwood_recieved_notes = selected_ironwood_notes
            .iter()
            .map(|note| {
                ReceivedNote::from_parts(
                    OutputRef::new(
                        OutputId::new(note.output_id().txid(), note.output_id().output_index()),
                        // The label must match the ReceivedNotes vector this
                        // note is returned in. Upstream never reads it, but it
                        // round-trips through the `exclude` split at the top of
                        // select_spendable_notes when the input selector prunes
                        // dust, and that split routes each ref by pool_type().
                        PoolType::IRONWOOD,
                    ),
                    note.output_id().txid(),
                    note.output_id()
                        .output_index()
                        .try_into()
                        .expect("shielded notes are always valid u16"),
                    *note.note(),
                    note.key_id().scope,
                    note.position()
                        .expect("note selection should filter on notes with positions"),
                    None, // mined_height. TODO: How should we use this here?
                    None, // max_shielding_input_height. TODO: How should we use this here?
                )
            })
            .collect::<Vec<_>>();

        Ok(ReceivedNotes::new(
            sapling_recieved_notes,
            orchard_recieved_notes,
            ironwood_recieved_notes,
        ))
    }

    fn get_account_metadata(
        &self,
        _account: Self::AccountId,
        _selector: &zcash_client_backend::data_api::NoteFilter,
        _target_height: TargetHeight,
        _exclude: &[Self::NoteRef],
    ) -> Result<zcash_client_backend::data_api::AccountMeta, Self::Error> {
        unimplemented!()
    }

    fn get_unspent_transparent_output(
        &self,
        _outpoint: &OutPoint,
        _target_height: TargetHeight,
    ) -> Result<Option<WalletTransparentOutput<Self::AccountId>>, Self::Error> {
        unimplemented!()
    }

    // fn get_spendable_transparent_outputs(
    //     &self,
    //     address: &TransparentAddress,
    //     target_height: TargetHeight,
    //     confirmations_policy: ConfirmationsPolicy,
    //     _output_filter: CoinbaseFilter,
    // ) -> Result<Vec<WalletUtxo>, Self::Error> {
    //     let address = transparent::encode_address(&self.chain_type, *address);

    //     // TODO: add recipient key scope metadata
    //     Ok(self
    //         .spendable_transparent_coins(
    //             target_height.into(),
    //             confirmations_policy.allow_zero_conf_shielding(),
    //             false,
    //         )
    //         .into_iter()
    //         .filter(|&output| output.address() == address)
    //         .filter_map(|output| {
    //             WalletTransparentOutput::from_parts(
    //                 output.output_id().into(),
    //                 TxOut::new(
    //                     output.value().try_into().expect("value from checked type"),
    //                     output.script().clone(),
    //                 ),
    //                 Some(
    //                     self.output_transaction(output)
    //                         .status()
    //                         .get_confirmed_height()
    //                         .expect("output must be confirmed in this scope"),
    //                 ),
    //             )
    //             .map(|transparent_output| WalletUtxo::new(transparent_output, None))
    //         })
    //         .collect())
    // }

    fn get_spendable_transparent_outputs(
        &self,
        address: &TransparentAddress,
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
        _output_filter: CoinbaseFilter,
    ) -> Result<Vec<WalletTransparentOutput<Self::AccountId>>, Self::Error> {
        let address = transparent::encode_address(&self.chain_type, *address);

        // TODO: add recipient key scope metadata
        Ok(self
            .spendable_transparent_coins(
                target_height.into(),
                confirmations_policy.allow_zero_conf_shielding(),
                false,
            )
            .into_iter()
            .filter(|&output| output.address() == address)
            .filter_map(|output| {
                WalletTransparentOutput::from_parts(
                    output.output_id().into(),
                    TxOut::new(
                        output.value().try_into().expect("value from checked type"),
                        output.script().clone(),
                    ),
                    Some(
                        self.output_transaction(output)
                            .status()
                            .get_confirmed_height()
                            .expect("output must be confirmed in this scope"),
                    ),
                    // TODO: populate recipient/funding account metadata once the
                    // wallet tracks per-output account attribution.
                    None,
                    None,
                    None,
                )
            })
            .collect())
    }

    fn select_unspent_notes(
        &self,
        _account: Self::AccountId,
        _sources: &[ShieldedPool],
        _target_height: TargetHeight,
        _exclude: &[Self::NoteRef],
    ) -> Result<ReceivedNotes<Self::NoteRef>, Self::Error> {
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    use zcash_client_backend::data_api::MaxSpendMode;
    use zcash_protocol::value::Zatoshis;

    use super::*;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

    /// `InputSource::select_spendable_notes` is driven by upstream
    /// `zcash_client_backend` (`propose_send_max`) with
    /// `TargetValue::AllFunds`. An unsupported request must surface
    /// through the trait's error channel (`Result<_, WalletError>`),
    /// never abort the process.
    #[test]
    fn select_spendable_notes_all_funds_returns_err_not_panic() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .sapling_note(100_000)
            .orchard_note(100_000)
            .ironwood_note(100_000)
            .build();

        let result = wallet.select_spendable_notes(
            zip32::AccountId::ZERO,
            TargetValue::AllFunds(MaxSpendMode::MaxSpendable),
            &[
                ShieldedPool::Sapling,
                ShieldedPool::Orchard,
                ShieldedPool::Ironwood,
            ],
            BlockHeight::from_u32(21).into(),
            ConfirmationsPolicy::MIN,
            &[],
        );

        assert!(
            result.is_err(),
            "TargetValue::AllFunds is unsupported: that must be reported \
             through the trait's error channel, not a panic; got {result:?}"
        );
    }

    /// Pins the resolution of the FIXME at the `PoolType::IRONWOOD` label in
    /// `InputSource::select_spendable_notes`: the code is right and the comment
    /// above it is stale.
    ///
    /// The `OutputRef` pool label is never read by the upstream proposal
    /// engine — pool-involvement accounting, bundle attribution, and fee
    /// calculation all dispatch on the embedded `Note` enum (stamped by
    /// `ReceivedNotes::into_vec` from the vector the note occupies), not on
    /// the `NoteRef`. The label's one consumer is this wallet's own `exclude`
    /// split at the top of `select_spendable_notes`: upstream's
    /// `GreedyInputSelector` hands previously selected refs back verbatim as
    /// `exclude` when the change strategy reports `ChangeError::DustInputs`,
    /// and the split routes each ref to a pool bucket by `pool_type()`. An
    /// ironwood note labeled `ORCHARD` (as the stale comment mandates) would
    /// land in the orchard bucket, never be excluded from ironwood selection,
    /// and be re-selected forever — the selector would then abort with
    /// `InsufficientFunds` instead of pruning the dust.
    #[test]
    fn ironwood_noteref_pool_label_round_trips_through_exclude() {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(100_000)
            .ironwood_note(100_000)
            .build();

        let confirmations_policy =
            ConfirmationsPolicy::new_symmetrical(1.try_into().expect("nonzero"), false);
        let target_height = TargetHeight::from(BlockHeight::from_u32(21));
        let sources = [
            ShieldedPool::Ironwood,
            ShieldedPool::Orchard,
            ShieldedPool::Sapling,
        ];

        // Phase 1: selection over ironwood-only funds labels every selected
        // ref IRONWOOD, matching the vector (and hence the Note-enum pool)
        // the note is returned in.
        let selected = wallet
            .select_spendable_notes(
                zip32::AccountId::ZERO,
                TargetValue::AtLeast(Zatoshis::const_from_u64(150_000)),
                &sources,
                target_height,
                confirmations_policy,
                &[],
            )
            .expect("selection over synthetic funds succeeds");
        assert_eq!(
            selected.ironwood().len(),
            2,
            "both fabricated ironwood notes are selected"
        );
        let refs: Vec<_> = selected
            .ironwood()
            .iter()
            .map(|note| *note.internal_note_id())
            .collect();
        for output_ref in &refs {
            assert_eq!(
                output_ref.pool_type(),
                PoolType::IRONWOOD,
                "an ironwood note's ref must carry the IRONWOOD label so the \
                 exclude split routes it back to the ironwood bucket"
            );
        }

        // Phase 2: the refs round-trip through `exclude` exactly as the
        // greedy input selector's dust-pruning path replays them. Correctly
        // labeled refs suppress re-selection; ORCHARD-labeled refs would fall
        // into the wrong bucket and the same notes would come back.
        let reselected = wallet
            .select_spendable_notes(
                zip32::AccountId::ZERO,
                TargetValue::AtLeast(Zatoshis::const_from_u64(150_000)),
                &sources,
                target_height,
                confirmations_policy,
                &refs,
            )
            .expect("selection with exclusions succeeds");
        assert!(
            reselected.ironwood().is_empty(),
            "excluded ironwood refs must not be re-selected; re-selection \
             means the exclude split routed them to the wrong pool bucket"
        );
    }
}

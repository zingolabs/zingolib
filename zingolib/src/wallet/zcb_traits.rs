use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    num::NonZeroU32,
    ops::Range,
};

use secrecy::SecretVec;
use shardtree::{ShardTree, error::ShardTreeError, store::ShardStore};
use zcash_client_backend::{
    data_api::{
        Account, AccountBirthday, AccountPurpose, Balance, BlockMetadata, CoinbaseFilter,
        InputSource, NullifierQuery, ORCHARD_SHARD_HEIGHT, OutputLockStore, ReceivedNotes,
        ReceivedTransactionOutput, SAPLING_SHARD_HEIGHT, TargetValue, TransactionDataRequest,
        TransparentKeyOrigin, WalletCommitmentTrees, WalletRead, WalletSummary, WalletWrite,
        Zip32Derivation,
        chain::{ChainState, CommitmentTreeRoot},
        error::FindAccountForAddressError,
        locking::{LockError, LockFilter, LockOwner},
        wallet::{ConfirmationsPolicy, TargetHeight},
    },
    wallet::{
        Exposure, NoteId, OutputRef as LockedOutputRef, ReceivedNote, TransparentAddressMetadata,
        WalletTransparentOutput,
    },
};
use zcash_keys::{address::UnifiedAddress, keys::UnifiedFullViewingKey};
use zcash_primitives::{
    block::BlockHash,
    transaction::{Transaction, TxId},
};
use zcash_protocol::{
    ShieldedPool,
    consensus::{self, BlockHeight, Parameters},
    memo::Memo,
};
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::bundle::{OutPoint, TxOut};
use zcash_transparent::keys::TransparentKeyScope;

use super::note_selection::{self, Selection};
use super::{LightWallet, error::WalletError, migration, output::OutputRef};
use pepper_sync::{
    error::SyncError,
    keys::{
        KeyId,
        transparent::{self, TransparentScope},
    },
    wallet::{
        IronwoodNote, KeyIdInterface, NoteInterface, OrchardNote, OrchardShardStore,
        OutputInterface, SaplingNote, SaplingShardStore, TransparentCoin, traits::SyncWallet,
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
                UnifiedFullViewingKey::try_from(*unified_key)
                    .is_ok_and(|account_ufvk| account_ufvk.is_equivalent_to(ufvk))
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

    fn get_wallet_recover_until(&self) -> Result<Option<BlockHeight>, Self::Error> {
        Ok(None)
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

        let Some(max_checkpoint_height) = self
            .shard_trees
            .sapling
            .store()
            .max_checkpoint_id()
            .expect("infallible")
        else {
            return Ok(None);
        };

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
                let address = crate::wallet::keys::transparent_address_from_encoded(
                    self.chain_type.network_type(),
                    encoded_address,
                )?;
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
                let address = crate::wallet::keys::transparent_address_from_encoded(
                    self.chain_type.network_type(),
                    encoded_address,
                )?;
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

    fn get_unspent_transparent_outpoints(
        &self,
    ) -> Result<HashMap<OutPoint, Self::AccountId>, Self::Error> {
        unimplemented!()
    }

    fn get_transparent_receiver_accounts(
        &self,
    ) -> Result<
        HashMap<TransparentAddress, (Self::AccountId, Option<TransparentKeyScope>)>,
        Self::Error,
    > {
        unimplemented!()
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

/// The account owning the wallet output `output` names, when the wallet holds it.
fn output_account<O: OutputInterface>(
    transaction: &pepper_sync::wallet::WalletTransaction,
    output: &LockedOutputRef,
) -> Option<zip32::AccountId> {
    (O::POOL_TYPE == output.pool())
        .then(|| {
            O::transaction_outputs(transaction)
                .iter()
                .find(|held| held.output_id().output_index() == output.output_index())
                .map(|held| held.key_id().account_id())
        })
        .flatten()
}

impl LightWallet {
    /// The account owning `output`, when the wallet holds an output matching it.
    fn locked_output_account(&self, output: &LockedOutputRef) -> Option<zip32::AccountId> {
        let transaction = self.wallet_transactions.get(output.txid())?;
        output_account::<SaplingNote>(transaction, output)
            .or_else(|| output_account::<OrchardNote>(transaction, output))
            .or_else(|| output_account::<IronwoodNote>(transaction, output))
            .or_else(|| output_account::<TransparentCoin>(transaction, output))
    }

    /// Every output this account has locked at `target_height`, the height a new
    /// transaction would be mined at.
    fn locked_outputs_of(
        &self,
        account: zip32::AccountId,
        target_height: BlockHeight,
    ) -> Vec<LockedOutputRef> {
        self.output_locks
            .iter()
            .filter(|(_, expiry_height)| *expiry_height >= target_height)
            .map(|(output, _)| output)
            .filter(|output| self.locked_output_account(output) == Some(account))
            .collect()
    }
}

impl OutputLockStore for LightWallet {
    type Error = WalletError;
    type AccountId = zip32::AccountId;

    fn lock_outputs(
        &mut self,
        outputs: &[LockedOutputRef],
        owner: LockOwner,
        lock_expiry_height: BlockHeight,
    ) -> Result<usize, LockError<Self::Error>> {
        let chain_tip = self
            .sync_state
            .last_known_chain_height()
            .unwrap_or_else(|| BlockHeight::from_u32(0));
        if let Some(unknown) = outputs
            .iter()
            .find(|output| self.locked_output_account(output).is_none())
        {
            return Err(LockError::LockFailure(*unknown));
        }
        self.output_locks
            .acquire(outputs, owner, lock_expiry_height, chain_tip)
            .map_err(LockError::LockFailure)
    }

    fn unlock_output(
        &mut self,
        output: &LockedOutputRef,
        owner: LockOwner,
    ) -> Result<bool, Self::Error> {
        Ok(self.output_locks.release(output, owner))
    }

    fn clear_locked_outputs(&mut self, account: Self::AccountId) -> Result<usize, Self::Error> {
        let held: Vec<LockedOutputRef> = self
            .output_locks
            .iter()
            .map(|(output, _)| output)
            .filter(|output| self.locked_output_account(output) == Some(account))
            .collect();
        Ok(held
            .iter()
            .filter(|output| self.output_locks.discard(output))
            .count())
    }

    fn get_locked_outputs(
        &self,
        account: Self::AccountId,
    ) -> Result<Vec<LockedOutputRef>, Self::Error> {
        let target_height = self
            .sync_state
            .last_known_chain_height()
            .map_or_else(|| BlockHeight::from_u32(0), |tip| tip + 1);
        Ok(self.locked_outputs_of(account, target_height))
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
    ) -> Result<
        (
            <Self as WalletRead>::AccountId,
            zcash_keys::keys::UnifiedSpendingKey,
        ),
        <Self as WalletRead>::Error,
    > {
        unimplemented!()
    }

    fn import_account_hd(
        &mut self,
        _account_name: &str,
        _seed: &SecretVec<u8>,
        _account_index: zip32::AccountId,
        _birthday: &AccountBirthday,
        _key_source: Option<&str>,
    ) -> Result<(Self::Account, zcash_keys::keys::UnifiedSpendingKey), <Self as WalletRead>::Error>
    {
        unimplemented!()
    }

    fn import_account_ufvk(
        &mut self,
        _account_name: &str,
        _unified_key: &UnifiedFullViewingKey,
        _birthday: &AccountBirthday,
        _purpose: AccountPurpose,
        _key_source: Option<&str>,
    ) -> Result<Self::Account, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn delete_account(
        &mut self,
        _account: <Self as WalletRead>::AccountId,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn set_account_name(
        &mut self,
        _account: <Self as WalletRead>::AccountId,
        _name: &str,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn get_next_available_address(
        &mut self,
        _account: <Self as WalletRead>::AccountId,
        _request: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<(UnifiedAddress, zip32::DiversifierIndex)>, <Self as WalletRead>::Error>
    {
        unimplemented!()
    }

    fn get_address_for_index(
        &mut self,
        _account: <Self as WalletRead>::AccountId,
        _diversifier_index: zip32::DiversifierIndex,
        _request: zcash_keys::keys::UnifiedAddressRequest,
    ) -> Result<Option<UnifiedAddress>, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn update_chain_tip(
        &mut self,
        _tip_height: BlockHeight,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn put_blocks(
        &mut self,
        _from_state: &zcash_client_backend::data_api::chain::ChainState,
        _blocks: Vec<zcash_client_backend::data_api::ScannedBlock<<Self as WalletRead>::AccountId>>,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn put_received_transparent_utxo(
        &mut self,
        _output: &WalletTransparentOutput<<Self as WalletRead>::AccountId>,
    ) -> Result<Self::UtxoRef, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn store_decrypted_tx(
        &mut self,
        _received_tx: zcash_client_backend::data_api::DecryptedTransaction<
            Transaction,
            <Self as WalletRead>::AccountId,
        >,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn queue_rescan(
        &mut self,
        _range: Range<BlockHeight>,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn prune_scan_queue_below(
        &mut self,
        _height: BlockHeight,
        _retain_with_priority: Option<zcash_client_backend::data_api::scanning::ScanPriority>,
    ) -> Result<u64, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn set_tx_trust(
        &mut self,
        _txid: TxId,
        _trusted: bool,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn store_transactions_to_be_sent(
        &mut self,
        transactions: &[zcash_client_backend::data_api::SentTransaction<
            <Self as WalletRead>::AccountId,
        >],
    ) -> Result<(), <Self as WalletRead>::Error> {
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

    fn truncate_to_height(
        &mut self,
        _max_height: BlockHeight,
    ) -> Result<BlockHeight, <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn truncate_to_chain_state(
        &mut self,
        _chain_state: ChainState,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn rewind_to_chain_state(
        &mut self,
        _chain_state: ChainState,
        _reset_account_birthdays: std::collections::HashSet<<Self as WalletRead>::AccountId>,
    ) -> Result<
        (),
        zcash_client_backend::data_api::error::RewindError<
            <Self as WalletRead>::AccountId,
            <Self as WalletRead>::Error,
        >,
    > {
        unimplemented!()
    }

    fn reserve_next_n_ephemeral_addresses(
        &mut self,
        account_id: <Self as WalletRead>::AccountId,
        n: usize,
    ) -> Result<Vec<(TransparentAddress, TransparentAddressMetadata)>, <Self as WalletRead>::Error>
    {
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
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }

    fn notify_address_checked(
        &mut self,
        _request: zcash_client_backend::data_api::TransactionsInvolvingAddress,
        _as_of_height: BlockHeight,
    ) -> Result<(), <Self as WalletRead>::Error> {
        unimplemented!()
    }
}

/// The root of `tree`'s completed subtree at `index`, or `None` while that subtree is incomplete.
fn subtree_root<S, H, C, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    tree: &ShardTree<S, DEPTH, SHARD_HEIGHT>,
    shard_height: u8,
    index: u64,
) -> Result<Option<H>, ShardTreeError<S::Error>>
where
    H: incrementalmerkletree::Hashable + Clone + PartialEq,
    C: Clone + std::fmt::Debug + Ord,
    S: ShardStore<H = H, CheckpointId = C>,
{
    let address = incrementalmerkletree::Address::from_parts(shard_height.into(), index);
    if !ShardTree::<S, DEPTH, SHARD_HEIGHT>::root_addr().contains(&address) {
        return Ok(None);
    }
    match tree.root(address, incrementalmerkletree::Position::from(u64::MAX)) {
        Ok(root) => Ok(Some(root)),
        Err(ShardTreeError::Query(_)) => Ok(None),
        Err(other) => Err(other),
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

    /// The stored root of the completed Sapling subtree at `index`, when the store holds one.
    fn get_sapling_subtree_root(
        &mut self,
        index: u64,
    ) -> Result<Option<sapling_crypto::Node>, ShardTreeError<Self::Error>> {
        subtree_root(&self.shard_trees.sapling, SAPLING_SHARD_HEIGHT, index)
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

    /// The stored root of the completed Orchard subtree at `index`, when the store holds one.
    fn get_orchard_subtree_root(
        &mut self,
        index: u64,
    ) -> Result<Option<orchard::tree::MerkleHashOrchard>, ShardTreeError<Self::Error>> {
        subtree_root(&self.shard_trees.orchard, ORCHARD_SHARD_HEIGHT, index)
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

    fn anchor_computable(
        &self,
        protocol: ShieldedPool,
        height: BlockHeight,
    ) -> Result<bool, Self::Error> {
        Ok(self.anchor_is_computable(protocol, height))
    }

    fn get_spendable_note(
        &self,
        _txid: &TxId,
        _protocol: ShieldedPool,
        _index: u32,
        _target_height: TargetHeight,
        _lock_filter: LockFilter<'_>,
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
        _lock_filter: LockFilter<'_>,
    ) -> Result<ReceivedNotes<Self::NoteRef>, Self::Error> {
        let (_, anchor_height) = self
            .get_target_and_anchor_heights(confirmations_policy.trusted())
            .expect("infallible")
            .ok_or(WalletError::NoSyncData)?;

        let reserved = self
            .migration
            .as_ref()
            .map(migration::MigrationState::reserved_output_ids)
            .unwrap_or_default();
        let unreserved = HashSet::new();
        let candidates = note_selection::Candidates {
            sapling: self.note_candidates(anchor_height, account, exclude, &unreserved)?,
            orchard: self.note_candidates(anchor_height, account, exclude, &reserved)?,
            ironwood: self.note_candidates(anchor_height, account, exclude, &unreserved)?,
        };
        let notes = match note_selection::select(target_value, sources, candidates) {
            Selection::Sufficient { notes }
            | Selection::Insufficient { notes }
            | Selection::AllSpendable { notes } => notes,
            Selection::EverythingUnsupported => {
                return Err(WalletError::AllFundsEverythingUnsupported);
            }
        };

        Ok(ReceivedNotes::new(
            received(notes.sapling),
            received(notes.orchard),
            received(notes.ironwood),
        ))
    }

    fn get_account_metadata(
        &self,
        _account: Self::AccountId,
        _selector: &zcash_client_backend::data_api::NoteFilter,
        _target_height: TargetHeight,
        _exclude: &[Self::NoteRef],
        _lock_filter: LockFilter<'_>,
    ) -> Result<zcash_client_backend::data_api::AccountMeta, Self::Error> {
        unimplemented!()
    }

    /// The pools an account's viewing key receives in, which is where the
    /// change strategies place its change.
    fn get_change_receivers(
        &self,
        account: Self::AccountId,
    ) -> Result<zcash_client_backend::data_api::ChangeReceivers, Self::Error> {
        let unified_key_store = self
            .unified_key_store
            .get(&account)
            .ok_or(super::error::KeyError::NoAccountKeys)?;
        let ufvk = UnifiedFullViewingKey::try_from(unified_key_store)?;
        Ok(zcash_client_backend::data_api::ChangeReceivers::from_uivk(
            &ufvk.to_unified_incoming_viewing_key(),
        ))
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
        _lock_filter: LockFilter<'_>,
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
        _lock_filter: LockFilter<'_>,
    ) -> Result<ReceivedNotes<Self::NoteRef>, Self::Error> {
        unimplemented!()
    }
}

fn received<N>(notes: Vec<N>) -> Vec<ReceivedNote<OutputRef, N::ZcashNote>>
where
    N: NoteInterface<KeyId = KeyId>,
    N::ZcashNote: Clone,
{
    notes
        .into_iter()
        .map(|note| {
            ReceivedNote::from_parts(
                OutputRef::new(note.output_id(), N::POOL_TYPE),
                note.output_id().txid(),
                note.output_id().output_index().try_into().expect(
                    "a shielded bundle holds fewer than 2^16 outputs, and output_index indexes one",
                ),
                note.note().clone(),
                note.key_id().scope,
                note.position().expect(
                    "spendable_notes admits only notes that hold a commitment tree position",
                ),
                None,
                None,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use zcash_client_backend::data_api::MaxSpendMode;
    use zcash_protocol::PoolType;
    use zcash_protocol::value::Zatoshis;

    use super::*;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

    /// One spendable note per shielded pool, plus an orchard note at the
    /// dust line (`MARGINAL_FEE`) that budgeted selection would also
    /// refuse to spend.
    fn funded_wallet() -> LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .sapling_note(50_000)
            .orchard_note(100_000)
            .ironwood_note(70_000)
            .orchard_note(5_000)
            .build()
    }

    fn select_all_funds(
        wallet: &LightWallet,
        mode: MaxSpendMode,
    ) -> Result<ReceivedNotes<OutputRef>, WalletError> {
        wallet.select_spendable_notes(
            zip32::AccountId::ZERO,
            TargetValue::AllFunds(mode),
            &[
                ShieldedPool::Ironwood,
                ShieldedPool::Orchard,
                ShieldedPool::Sapling,
            ],
            TargetHeight::from(21),
            ConfirmationsPolicy::new_symmetrical(NonZeroU32::new(1).expect("nonzero"), false),
            &[],
            LockFilter::Unfiltered,
        )
    }

    /// A funded wallet whose sapling shard tree holds no checkpoint.
    fn wallet_without_sapling_checkpoint() -> LightWallet {
        use pepper_sync::sync::SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE;
        use shardtree::store::memory::MemoryShardStore;

        let mut wallet = funded_wallet();
        wallet.shard_trees.sapling = ShardTree::new(
            MemoryShardStore::empty(),
            SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE as usize,
        );
        assert!(wallet.sync_state.last_known_chain_height().is_some());
        wallet
    }

    #[test]
    fn missing_sapling_checkpoint_means_no_anchor_and_zero_balance() {
        let wallet = wallet_without_sapling_checkpoint();

        assert_eq!(
            wallet
                .get_target_and_anchor_heights(wallet.wallet_settings.min_confirmations)
                .expect("infallible"),
            None
        );
        assert_eq!(
            wallet
                .shielded_spendable_balance(zip32::AccountId::ZERO, false)
                .expect("a missing anchor is a zero balance, not an error"),
            Zatoshis::ZERO
        );
    }

    #[test]
    fn reload_restores_missing_sapling_checkpoint() {
        let mut wallet = wallet_without_sapling_checkpoint();

        wallet.save_required = true;
        let bytes = wallet.save().unwrap().expect("save required");
        let reloaded = LightWallet::read(bytes.as_slice(), wallet.chain_type()).unwrap();

        assert_eq!(
            reloaded
                .shard_trees
                .sapling
                .store()
                .max_checkpoint_id()
                .expect("infallible"),
            Some(BlockHeight::from_u32(0))
        );
        assert!(
            reloaded
                .get_target_and_anchor_heights(wallet.wallet_settings.min_confirmations)
                .expect("infallible")
                .is_some()
        );
    }

    #[test]
    fn all_funds_max_spendable_selects_every_spend_worthy_note() {
        let notes = select_all_funds(&funded_wallet(), MaxSpendMode::MaxSpendable)
            .expect("all-funds selection must succeed on a funded, synced wallet");
        assert_eq!(notes.sapling().len(), 1);
        assert_eq!(
            notes.orchard().len(),
            1,
            "the dust orchard note must be left behind"
        );
        assert_eq!(notes.ironwood().len(), 1);
        let total: u64 = notes
            .sapling()
            .iter()
            .map(|note| note.note().value().inner())
            .chain(
                notes
                    .orchard()
                    .iter()
                    .map(|note| note.note().value().inner()),
            )
            .chain(
                notes
                    .ironwood()
                    .iter()
                    .map(|note| note.note().value().inner()),
            )
            .sum();
        assert_eq!(total, 220_000);
    }

    #[test]
    fn all_funds_everything_is_a_typed_error_not_an_abort() {
        let error = select_all_funds(&funded_wallet(), MaxSpendMode::Everything)
            .expect_err("Everything mode is unimplemented and must refuse, not abort");
        assert!(
            matches!(error, WalletError::AllFundsEverythingUnsupported),
            "the refusal must be the dedicated typed error, observed: {error}"
        );
    }

    // Tests that selection returns the requested pools' notes alone when they fall short. Upstream
    // input selection rejects a note from a pool it did not ask for
    // (GreedyInputSelectorError::UnrequestedPoolSelected) and reports the shortfall itself.
    #[test]
    fn at_least_selection_stays_in_the_requested_pools_when_they_fall_short() {
        let notes = funded_wallet()
            .select_spendable_notes(
                zip32::AccountId::ZERO,
                TargetValue::AtLeast(Zatoshis::const_from_u64(200_000)),
                &[ShieldedPool::Ironwood, ShieldedPool::Orchard],
                TargetHeight::from(21),
                ConfirmationsPolicy::new_symmetrical(
                    NonZeroU32::new(1).expect("one is nonzero"),
                    false,
                ),
                &[],
                LockFilter::Unfiltered,
            )
            .expect("selection over synthetic funds succeeds");
        assert!(notes.sapling().is_empty());
        assert_eq!(notes.ironwood().len(), 1);
        assert_eq!(notes.orchard().len(), 1);
    }

    // Tests that all-funds selection gathers the requested pools alone.
    #[test]
    fn all_funds_selection_gathers_only_the_requested_pools() {
        let notes = funded_wallet()
            .select_spendable_notes(
                zip32::AccountId::ZERO,
                TargetValue::AllFunds(MaxSpendMode::MaxSpendable),
                &[ShieldedPool::Orchard],
                TargetHeight::from(21),
                ConfirmationsPolicy::new_symmetrical(
                    NonZeroU32::new(1).expect("one is nonzero"),
                    false,
                ),
                &[],
                LockFilter::Unfiltered,
            )
            .expect("all-funds selection succeeds on a funded, synced wallet");
        assert!(notes.sapling().is_empty());
        assert!(notes.ironwood().is_empty());
        assert_eq!(notes.orchard().len(), 1);
    }

    /// Pins the resolution of the FIXME at the `PoolType::IRONWOOD` label in
    /// `InputSource::select_spendable_notes`: the code is right and the comment
    /// above it is stale.
    ///
    /// The `OutputRef` pool label is never read by the upstream proposal
    /// engine, since pool-involvement accounting, bundle attribution, and fee
    /// calculation all dispatch on the embedded `Note` enum (stamped by
    /// `ReceivedNotes::into_vec` from the vector the note occupies), not on
    /// the `NoteRef`. The label's one consumer is this wallet's own `exclude`
    /// split at the top of `select_spendable_notes`: upstream's
    /// `GreedyInputSelector` hands previously selected refs back verbatim as
    /// `exclude` when the change strategy reports `ChangeError::DustInputs`,
    /// and the split routes each ref to a pool bucket by `pool_type()`. An
    /// ironwood note labeled `ORCHARD` (as the stale comment mandates) would
    /// land in the orchard bucket, never be excluded from ironwood selection,
    /// and be re-selected forever, so the selector would then abort with
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
                LockFilter::Unfiltered,
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
                LockFilter::Unfiltered,
            )
            .expect("selection with exclusions succeeds");
        assert!(
            reselected.ironwood().is_empty(),
            "excluded ironwood refs must not be re-selected; re-selection \
             means the exclude split routed them to the wrong pool bucket"
        );
    }
}

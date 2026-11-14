//! Core module, containing `crate::wallet::LightWallet` with methods for all wallet functionality.

use std::collections::{BTreeMap, HashMap};

use bip0039::Mnemonic;

use pepper_sync::sync::MAX_REORG_ALLOWANCE;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zcash_transparent::keys::NonHardenedChildIndex;

use pepper_sync::keys::transparent::{TransparentScope, encode_address};
use pepper_sync::wallet::{KeyIdInterface, PoolActivation, ScanTarget, ShardTrees};
use pepper_sync::{
    keys::transparent::TransparentAddressId,
    wallet::{NullifierMap, OutputId, SyncState, WalletBlock, WalletTransaction},
};
use zl_common::price::PriceList;

use crate::config::{ChainType, WalletConfig};
use crate::data::proposal::ZingoProposal;
use error::{KeyError, WalletError};
use keys::unified::{UnifiedAddressId, UnifiedKeyStore};

pub mod error;
pub mod expiry;
pub mod traits;
pub mod utils;

// these mods contain pieces of the impl LightWallet
pub mod balance;
pub mod disk;
pub mod invariants;
pub mod keys;
pub mod locks;
pub mod migration;
pub(crate) mod note_selection;
pub mod output;
pub mod propose;
pub mod send;
pub mod summary;
pub mod sync;
pub mod transaction;
pub mod transparent;
mod zcb_traits;

pub use pepper_sync::config::{
    PerformanceLevel, SyncConfig, TransparentAddressDiscovery, TransparentAddressDiscoveryScopes,
};
pub use zl_file_format::{RecoveryInfo, WalletSettings};

/// Base data required to construct a new [`crate::wallet::LightWallet`].
pub(crate) struct WalletBase {
    pub(crate) unified_key_store: BTreeMap<zip32::AccountId, UnifiedKeyStore>,
    pub(crate) mnemonic: Option<Mnemonic>,
    pub(crate) birthday: BlockHeight,
    pub(crate) wallet_settings: WalletSettings,
}

/// In-memory wallet data struct
///
/// The `mnemonic` can be `None` in the case of a wallet created directly from UFVKs or USKs.
///
/// As no relevant transactions related to this wallet will exist below the wallet's birthday, sync will start from
/// `birthday` block height.
///
/// When wallet state is changed due to sync, send or creating addresses, `save_required` will be set to `true`
/// automatically. See [`crate::lightclient::LightClient::save_task`] and related methods to persist the wallet.
#[derive(Debug)]
pub struct LightWallet {
    /// Current wallet version.
    current_version: u64,
    /// Wallet version that was read from on wallet load.
    read_version: u64,
    /// Blockchain network type
    chain_type: ChainType,
    /// The seed for the wallet, stored as a zip339 Mnemonic, and the account index.
    mnemonic: Option<Mnemonic>,
    /// The block height at which the wallet was created.
    birthday: BlockHeight,
    /// - Process-lifetime state, never written to the wallet file.
    birthday_adjusted_from: Option<BlockHeight>,
    /// Unified key store
    pub unified_key_store: BTreeMap<zip32::AccountId, UnifiedKeyStore>,
    /// `Unified_addresses`
    unified_addresses: BTreeMap<UnifiedAddressId, UnifiedAddress>,
    /// Transparent addresses
    transparent_addresses: BTreeMap<TransparentAddressId, String>,
    /// Wallet blocks
    pub wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    /// Wallet transactions
    pub wallet_transactions: HashMap<TxId, WalletTransaction>,
    /// Nullifier map
    pub nullifier_map: NullifierMap,
    /// Outpoint map
    pub outpoint_map: BTreeMap<OutputId, ScanTarget>,
    /// Shard trees
    pub shard_trees: ShardTrees,
    /// Sync state
    pub sync_state: SyncState,
    /// Wallet settings
    ///
    /// Altering the sync config will not automatically restart sync which is needed for the changes to take effect.
    /// It is recommended to rescan after altering the transparent address discovery settings as scanned ranges will
    /// have been scanned with the previous configuration.
    pub wallet_settings: WalletSettings,
    /// The current and historical daily price of zec.
    pub price_list: PriceList,
    /// Orchard→Ironwood migration state, present while a migration is
    /// planned or in flight. Wallet-file-local by design: restore-from-seed
    /// starts fresh.
    pub migration: Option<migration::MigrationState>,
    /// Send proposal
    send_proposal: Option<ZingoProposal>,
    /// Advisory output locks reserving an in-flight proposal's inputs. Process
    /// -lifetime state beside `send_proposal`, never written to the wallet file.
    output_locks: locks::OutputLocks,
    /// Boolean for tracking whether the wallet state has changed since last save.
    pub(crate) save_required: bool,
}

impl LightWallet {
    /// Create a new in-memory wallet from [`crate::config::WalletConfig`].
    ///
    /// # Error
    ///
    /// An error will be returned if the wallet config is of `Read` variant as the wallet has already been created.
    /// If is the responsbility of the struct that owns the [`crate::wallet::LightWallet`] to use the
    /// `LightWallet::read` method instead.
    #[allow(clippy::result_large_err)]
    pub fn new(chain_type: ChainType, wallet_config: WalletConfig) -> Result<Self, WalletError> {
        let wallet_base = wallet_config.resolve(chain_type)?;
        Self::from_base(chain_type, wallet_base)
    }

    /// Construct a wallet from [`crate::wallet::WalletBase`], resolved from a [`crate::config::WalletConfig`].
    #[allow(clippy::result_large_err)]
    pub(crate) fn from_base(
        chain_type: ChainType,
        wallet_base: WalletBase,
    ) -> Result<Self, WalletError> {
        let WalletBase {
            unified_key_store,
            mnemonic,
            birthday,
            wallet_settings,
        } = wallet_base;

        let unified_key = unified_key_store
            .get(&zip32::AccountId::ZERO)
            .expect("account 0 must exist");
        let mut unified_addresses = BTreeMap::new();
        if let Some(receivers) = unified_key.default_receivers() {
            let unified_address_id = UnifiedAddressId {
                account_id: zip32::AccountId::ZERO,
                address_index: 0,
            };
            let first_unified_address = unified_key
                .generate_unified_address(unified_address_id.address_index, receivers)?;
            unified_addresses.insert(unified_address_id, first_unified_address.clone());
        }

        let mut transparent_addresses = BTreeMap::new();
        let transparent_address_id = TransparentAddressId::new(
            zip32::AccountId::ZERO,
            TransparentScope::External,
            NonHardenedChildIndex::ZERO,
        );
        match unified_key.generate_transparent_address(
            transparent_address_id.address_index(),
            transparent_address_id.scope(),
        ) {
            Ok(first_transparent_address) => {
                transparent_addresses.insert(
                    transparent_address_id,
                    encode_address(&chain_type, first_transparent_address),
                );
            }
            Err(KeyError::NoViewCapability) => (),
            Err(e) => return Err(e.into()),
        }

        let mut wallet = Self {
            current_version: LightWallet::serialized_version(),
            read_version: LightWallet::serialized_version(),
            chain_type,
            mnemonic,
            birthday: BlockHeight::from_u32(birthday.into()),
            birthday_adjusted_from: None,
            unified_key_store,
            unified_addresses,
            transparent_addresses,
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: ShardTrees::new(),
            sync_state: SyncState::new(),
            wallet_settings,
            price_list: PriceList::new(),
            migration: None,
            save_required: true,
            send_proposal: None,
            output_locks: locks::OutputLocks::default(),
        };
        wallet.raise_birthday_to_sapling_activation();
        Ok(wallet)
    }

    /// - Raises `birthday` to the chain's Sapling activation height and keeps the given height in `birthday_adjusted_from`.
    /// - Sets `save_required` when it raises.
    fn raise_birthday_to_sapling_activation(&mut self) {
        let Some(sapling_activation) =
            PoolActivation::of(&self.chain_type, zcash_protocol::ShieldedPool::Sapling)
        else {
            return;
        };
        let sapling_activation_height = sapling_activation.height();
        if self.birthday < sapling_activation_height {
            self.birthday_adjusted_from = Some(self.birthday);
            self.birthday = sapling_activation_height;
            self.save_required = true;
        }
    }

    /// Returns current wallet version.
    #[must_use]
    pub fn current_version(&self) -> u64 {
        self.current_version
    }

    /// Returns wallet version that was read from on wallet load.
    #[must_use]
    pub fn read_version(&self) -> u64 {
        self.read_version
    }

    /// Returns wallet birthday height.
    #[must_use]
    pub fn birthday(&self) -> BlockHeight {
        self.birthday
    }

    #[must_use]
    pub fn birthday_adjusted_from(&self) -> Option<BlockHeight> {
        self.birthday_adjusted_from
    }

    /// Returns chain type wallet is connected to.
    #[must_use]
    pub fn chain_type(&self) -> ChainType {
        self.chain_type
    }

    /// Returns the wallet's mnemonic for internal operations.
    #[must_use]
    pub(crate) fn mnemonic(&self) -> Option<&Mnemonic> {
        self.mnemonic.as_ref()
    }

    /// Returns the wallet's mnemonic phrase.
    #[must_use]
    pub fn mnemonic_phrase(&self) -> Option<String> {
        self.mnemonic().map(|m| m.phrase().to_string())
    }

    /// Returns unified addresses.
    #[must_use]
    pub fn unified_addresses(&self) -> &BTreeMap<UnifiedAddressId, UnifiedAddress> {
        &self.unified_addresses
    }

    /// Returns unified addresses in a JSON array.
    #[must_use]
    pub fn unified_addresses_json(&self) -> json::JsonValue {
        json::JsonValue::Array(
            self.unified_addresses
                .iter()
                .map(|(id, unified_address)| {
                    json::object! {
                        "account" => u32::from(id.account_id),
                        "address_index" => id.address_index,
                        "has_orchard" => unified_address.has_orchard(),
                        "has_sapling" => unified_address.has_sapling(),
                        "has_transparent" => unified_address.has_transparent(),
                        "encoded_address" => unified_address.encode(&self.chain_type),
                    }
                })
                .collect::<Vec<_>>(),
        )
    }

    /// Returns transparent addresses.
    #[must_use]
    pub fn transparent_addresses(&self) -> &BTreeMap<TransparentAddressId, String> {
        &self.transparent_addresses
    }

    /// Returns transparent addresses as mutable reference.
    #[must_use]
    pub(crate) fn transparent_addresses_mut(
        &mut self,
    ) -> &mut BTreeMap<TransparentAddressId, String> {
        &mut self.transparent_addresses
    }

    /// Returns transparent addresses in a JSON array.
    #[must_use]
    pub fn transparent_addresses_json(&self) -> json::JsonValue {
        json::JsonValue::Array(
            self.transparent_addresses
                .iter()
                .map(|(id, transparent_address)| {
                    json::object! {
                        "account" => u32::from(id.account_id()),
                        "address_index" => id.address_index().index(),
                        "scope" => id.scope().to_string(),
                        "encoded_address" => transparent_address.clone(),
                    }
                })
                .collect::<Vec<_>>(),
        )
    }

    /// Clears the proposal in the `send_proposal` field.
    pub fn clear_proposal(&mut self) {
        self.send_proposal = None;
    }

    /// Marks the wallet as having unsaved changes, scheduling the next [`crate::lightclient::LightClient::save_task`] tick to persist it.
    pub fn mark_dirty(&mut self) {
        self.save_required = true;
    }

    #[must_use]
    pub fn recovery_info(&self) -> Option<RecoveryInfo> {
        Some(RecoveryInfo {
            seed_phrase: self.mnemonic_phrase()?,
            birthday: self.birthday.into(),
            no_of_accounts: self.unified_key_store.len() as u32,
            birthday_adjusted_from: self.birthday_adjusted_from.map(u64::from),
        })
    }

    #[allow(clippy::result_large_err)]
    pub fn create_new_account(&mut self) -> Result<(), WalletError> {
        let last_account = self.unified_key_store.keys().copied().max();
        let account_id = last_account.map_or(Ok(zip32::AccountId::ZERO), |last_account| {
            last_account
                .next()
                .ok_or(WalletError::AccountCreationFailed)
        })?;
        self.unified_key_store.insert(
            account_id,
            UnifiedKeyStore::new_from_mnemonic(
                self.chain_type(),
                self.mnemonic().ok_or(WalletError::MnemonicNotFound)?,
                account_id,
            )?,
        );
        self.save_required = true;

        Ok(())
    }

    /// If the wallet state has changed since last save, serializes the wallet and returns the wallet bytes.
    /// Returns `Ok(None)` if the wallet state has not changed and save is not required.
    /// Returns error if serialization fails.
    ///
    /// Intended to be called from a save task which calls `save` in a loop, awaiting the wallet lock and checking
    /// `self.save_required` status, writing the returned wallet bytes to persistance.
    /// `save_required` field must be manually set back to false after wallet data has been successfully persisted to disk.
    pub fn save(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.save_required {
            let chain_type = self.chain_type;
            let mut wallet_bytes: Vec<u8> = vec![];
            self.write(&mut wallet_bytes, &chain_type)?;

            Ok(Some(wallet_bytes))
        } else {
            Ok(None)
        }
    }

    /// Prunes historical prices to days containing transactions in the wallet.
    ///
    /// Avoids pruning above fully scanned height.
    // TODO: under development
    pub fn prune_price_list(&mut self) {
        let Some(fully_scanned_height) = self.sync_state.fully_scanned_height() else {
            return;
        };
        let transaction_times = self
            .wallet_transactions
            .values()
            .filter(|transaction| {
                transaction
                    .status()
                    .get_confirmed_height()
                    .is_some_and(|height| height <= fully_scanned_height)
            })
            .map(pepper_sync::wallet::WalletTransaction::datetime)
            .collect();

        let prune_below = self
            .wallet_blocks
            .get(&fully_scanned_height)
            .expect("fully scanned height should always be on a scan range boundary")
            .time();
        self.price_list.prune(transaction_times, prune_below);
    }

    /// Clears all wallet data obtained from the block chain including the sync state.
    ///
    /// Adds scan targets to the new sync state to prioritise scanning relevant parts of the chain on rescan.
    /// Addresses are not cleared.
    ///
    /// Calling this function while the wallet is syncing will cause sync errors and require an additional clear while
    /// sync is stopped to recover.
    pub fn clear_all(&mut self) {
        let chain_height_opt = self.sync_state.last_known_chain_height();
        self.sync_state = SyncState::new();
        if let Some(chain_height) = chain_height_opt {
            pepper_sync::add_scan_targets(
                &mut self.sync_state,
                &self
                    .wallet_transactions
                    .values()
                    .filter(|transaction| {
                        transaction
                            .status()
                            .is_confirmed_before(&(chain_height - MAX_REORG_ALLOWANCE))
                    })
                    .filter_map(|transaction| {
                        transaction
                            .status()
                            .get_confirmed_height()
                            .map(|height| ScanTarget::new(height, Some(transaction.txid())))
                    })
                    .collect::<Vec<_>>(),
            );
        }

        self.wallet_blocks.clear();
        self.wallet_transactions.clear();
        self.nullifier_map.clear();
        self.outpoint_map.clear();
        self.shard_trees = ShardTrees::new();
        self.price_list = PriceList::new();

        self.save_required = true;
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod shardtree_boundary;

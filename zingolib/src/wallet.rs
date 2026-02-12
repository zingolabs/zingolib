//! Core module, containing `crate::wallet::LightWallet` with methods for all wallet functionality.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;

use bip0039::Mnemonic;

use zcash_client_backend::tor;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::legacy::keys::NonHardenedChildIndex;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use pepper_sync::keys::transparent::{self, TransparentScope};
use pepper_sync::wallet::{KeyIdInterface, ScanTarget, ShardTrees};
use pepper_sync::{
    keys::transparent::TransparentAddressId,
    wallet::{NullifierMap, OutputId, SyncState, WalletBlock, WalletTransaction},
};
use zingo_price::PriceList;

use crate::config::ChainType;
use error::{KeyError, PriceError, WalletError};
use keys::unified::{UnifiedAddressId, UnifiedKeyStore};
use send::SendProgress;

pub mod error;
pub(crate) mod legacy;
pub mod traits;
pub mod utils;

// these mods contain pieces of the impl LightWallet
pub mod balance;
pub mod disk;
pub mod keys;
pub mod output;
pub mod propose;
pub mod send;
pub mod summary;
pub mod sync;
pub mod transaction;
mod zcb_traits;

/// Wallet settings.
#[derive(Debug, Clone)]
pub struct WalletSettings {
    /// Sync configuration.
    pub sync_config: pepper_sync::config::SyncConfig,
    /// Minimum confirmations.
    pub min_confirmations: NonZeroU32,
}

/// Provides necessary information to recover the wallet without the wallet file.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct RecoveryInfo {
    /// 24-word mnemonic phrase.
    pub seed_phrase: String,
    /// Block height wallet was created.
    pub birthday: u64,
    /// Number of accounts in use.
    pub no_of_accounts: u32,
}

impl std::fmt::Display for RecoveryInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Wallet backup info:
{{
    seed phrase: {}
    birthday: {}
    no_of_accounts: {}
}}",
            self.seed_phrase, self.birthday, self.no_of_accounts,
        )
    }
}

/// Data used to initialize new instance of `LightWallet`
pub enum WalletBase {
    /// Generate a wallet with a new seed for a number of accounts.
    FreshEntropy { no_of_accounts: NonZeroU32 },
    /// Generate a wallet from a mnemonic (phrase or entropy) for a number of accounts.
    Mnemonic {
        mnemonic: Mnemonic,
        no_of_accounts: NonZeroU32,
    },
    /// Generate a wallet from a unified full viewing key.
    // TODO: take concrete UFVK type
    Ufvk(String),
    /// Generate a wallet from a unified spending key.
    // TODO: take concrete USK type
    Usk(Vec<u8>),
}

/// In-memory wallet data struct
///
/// The `mnemonic` can be `None` in the case of a wallet created directly from UFVKs or USKs.
///
/// As no relevant transactions related to this wallet will exist below the wallet's birthday, sync will start from
/// `birthday` block height.
///
/// When wallet state is changed due to sync, send or creating addresses, `save_required` will be set to `true`
/// automatically. Calling [`crate::wallet::LightWallet::save`] will serialize the wallet and reset `save_required`
/// to false, returning the bytes to be persisted. Also see [`crate::lightclient::LightClient::save_task`] and related
/// methods for a save task implementation.
#[derive(Debug)]
pub struct LightWallet {
    /// Current wallet version.
    current_version: u64,
    /// Wallet version that was read from on wallet load.
    read_version: u64,
    /// Network type
    pub network: ChainType,
    /// The seed for the wallet, stored as a zip339 Mnemonic, and the account index.
    mnemonic: Option<Mnemonic>,
    /// The block height at which the wallet was created.
    pub birthday: BlockHeight,
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
    /// Wallet settings.
    pub wallet_settings: WalletSettings,
    /// The current and historical daily price of zec.
    pub price_list: PriceList,
    /// Progress of an outgoing transaction
    pub send_progress: SendProgress,
    /// Boolean for tracking whether the wallet state has changed since last save.
    pub save_required: bool,
}

impl LightWallet {
    /// Create a new in-memory wallet.
    ///
    /// For wallets from fresh entropy, it is worth considering setting `birthday` to 100 blocks below current height
    /// of block chain to protect from re-orgs.
    #[allow(clippy::result_large_err)]
    pub fn new(
        network: ChainType,
        wallet_base: WalletBase,
        birthday: BlockHeight,
        wallet_settings: WalletSettings,
    ) -> Result<Self, WalletError> {
        let (unified_key_store, mnemonic) = match wallet_base {
            WalletBase::FreshEntropy { no_of_accounts } => {
                return Self::new(
                    network,
                    WalletBase::Mnemonic {
                        mnemonic: Mnemonic::generate(bip0039::Count::Words24),
                        no_of_accounts,
                    },
                    birthday,
                    wallet_settings,
                );
            }
            WalletBase::Mnemonic {
                mnemonic,
                no_of_accounts,
            } => {
                let no_of_accounts = u32::from(no_of_accounts);
                let unified_key_store = (0..no_of_accounts)
                    .map(|account_index| {
                        let account_id = zip32::AccountId::try_from(account_index)?;
                        Ok((
                            account_id,
                            UnifiedKeyStore::new_from_mnemonic(&network, &mnemonic, account_id)?,
                        ))
                    })
                    .collect::<Result<BTreeMap<_, _>, KeyError>>()?;
                (unified_key_store, Some(mnemonic))
            }
            WalletBase::Ufvk(ufvk_encoded) => {
                let mut unified_key_store = BTreeMap::new();
                unified_key_store.insert(
                    zip32::AccountId::ZERO,
                    UnifiedKeyStore::new_from_ufvk(&network, ufvk_encoded)?,
                );
                (unified_key_store, None)
            }
            WalletBase::Usk(unified_spending_key) => {
                let mut unified_key_store = BTreeMap::new();
                unified_key_store.insert(
                    zip32::AccountId::ZERO,
                    UnifiedKeyStore::new_from_usk(unified_spending_key.as_slice())?,
                );
                (unified_key_store, None)
            }
        };

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
                    transparent::encode_address(&network, first_transparent_address),
                );
            }
            Err(KeyError::NoViewCapability) => (),
            Err(e) => return Err(e.into()),
        }

        Ok(Self {
            current_version: LightWallet::serialized_version(),
            read_version: LightWallet::serialized_version(),
            network,
            mnemonic,
            birthday: BlockHeight::from_u32(birthday.into()),
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
            save_required: true,
            send_progress: SendProgress::new(0),
        })
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

    /// Returns the wallet's mnemonic (seed and phrase).
    #[must_use]
    pub fn mnemonic(&self) -> Option<&Mnemonic> {
        self.mnemonic.as_ref()
    }

    /// Returns the wallet's mnemonic phrase.
    #[must_use]
    pub fn mnemonic_phrase(&self) -> Option<String> {
        self.mnemonic()
            .map(|mnemonic| mnemonic.phrase().to_string())
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
                        "encoded_address" => unified_address.encode(&self.network),
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

    #[must_use]
    pub fn recovery_info(&self) -> Option<RecoveryInfo> {
        Some(RecoveryInfo {
            seed_phrase: self.mnemonic_phrase()?,
            birthday: self.birthday.into(),
            no_of_accounts: self.unified_key_store.len() as u32,
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
                &self.network,
                self.mnemonic().ok_or(WalletError::MnemonicNotFound)?,
                account_id,
            )?,
        );

        Ok(())
    }

    // Set the previous send's result as a JSON string.
    pub(super) fn set_send_result(&mut self, result: String) {
        self.send_progress.is_send_in_progress = false;
        self.send_progress.last_result = Some(result);
    }

    /// If the wallet state has changed since last save, serializes the wallet and returns the wallet bytes.
    /// Returns `Ok(None)` if the wallet state has not changed and save is not required.
    /// Returns error if serialization fails.
    ///
    /// Intended to be called from a save task which calls `save` in a loop, awaiting the wallet lock and checking
    /// `self.save_required` status, writing the returned wallet bytes to persistance.
    pub fn save(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.save_required {
            let network = self.network;
            let mut wallet_bytes: Vec<u8> = vec![];
            self.write(&mut wallet_bytes, &network)?;
            self.save_required = false;
            Ok(Some(wallet_bytes))
        } else {
            Ok(None)
        }
    }

    /// Update and return current price of ZEC.
    ///
    /// Will fetch via tor if a `tor_client` is provided.
    /// Currently only USD is supported.
    pub async fn update_current_price(
        &mut self,
        tor_client: Option<&tor::Client>,
    ) -> Result<f32, PriceError> {
        let current_price = self
            .price_list
            .update_current_price(tor_client)
            .await?
            .price_usd;
        self.save_required = true;

        Ok(current_price)
    }

    /// Updates historical daily price list.
    /// Prunes any unused price data in the wallet after it's been updated.
    /// If this is the first time update has been called, initialises the price list from the wallet data.
    ///
    /// Currently only USD is supported.
    // TODO: under development
    pub async fn update_historical_prices(&mut self) -> Result<(), PriceError> {
        if self
            .price_list
            .time_historical_prices_last_updated()
            .is_none()
        {
            let Some(birthday) = self.sync_state.wallet_birthday() else {
                return Err(PriceError::NotInitialised);
            };
            let birthday_block = match self.wallet_blocks.get(&birthday) {
                Some(block) => block.clone(),
                None => {
                    return Err(PriceError::NotInitialised);
                }
            };
            self.price_list.set_start_time(birthday_block.time());
        }
        self.price_list.update_historical_price_list().await?;
        self.prune_price_list();
        self.save_required = true;

        todo!()
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
    pub fn clear_all(&mut self) {
        self.sync_state = SyncState::new();
        pepper_sync::add_scan_targets(
            &mut self.sync_state,
            &self
                .wallet_transactions
                .values()
                .filter_map(|transaction| {
                    transaction
                        .status()
                        .get_confirmed_height()
                        .map(|height| ScanTarget {
                            block_height: height,
                            txid: transaction.txid(),
                            narrow_scan_area: true,
                        })
                })
                .collect::<Vec<_>>(),
        );

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
mod tests {
    use incrementalmerkletree::frontier::CommitmentTree;
    use orchard::tree::MerkleHashOrchard;

    // TODO: move to relevant mod
    #[test]
    fn anchor_from_tree_works() {
        // These commitment values copied from zcash/orchard, and were originally derived from the bundle
        // data that was generated for testing commitment tree construction inside of zcashd here.
        // https://github.com/zcash/zcash/blob/ecec1f9769a5e37eb3f7fd89a4fcfb35bc28eed7/src/test/data/merkle_roots_orchard.h

        let commitments = [
            [
                0x68, 0x13, 0x5c, 0xf4, 0x99, 0x33, 0x22, 0x90, 0x99, 0xa4, 0x4e, 0xc9, 0x9a, 0x75,
                0xe1, 0xe1, 0xcb, 0x46, 0x40, 0xf9, 0xb5, 0xbd, 0xec, 0x6b, 0x32, 0x23, 0x85, 0x6f,
                0xea, 0x16, 0x39, 0x0a,
            ],
            [
                0x78, 0x31, 0x50, 0x08, 0xfb, 0x29, 0x98, 0xb4, 0x30, 0xa5, 0x73, 0x1d, 0x67, 0x26,
                0x20, 0x7d, 0xc0, 0xf0, 0xec, 0x81, 0xea, 0x64, 0xaf, 0x5c, 0xf6, 0x12, 0x95, 0x69,
                0x01, 0xe7, 0x2f, 0x0e,
            ],
            [
                0xee, 0x94, 0x88, 0x05, 0x3a, 0x30, 0xc5, 0x96, 0xb4, 0x30, 0x14, 0x10, 0x5d, 0x34,
                0x77, 0xe6, 0xf5, 0x78, 0xc8, 0x92, 0x40, 0xd1, 0xd1, 0xee, 0x17, 0x43, 0xb7, 0x7b,
                0xb6, 0xad, 0xc4, 0x0a,
            ],
            [
                0x9d, 0xdc, 0xe7, 0xf0, 0x65, 0x01, 0xf3, 0x63, 0x76, 0x8c, 0x5b, 0xca, 0x3f, 0x26,
                0x46, 0x60, 0x83, 0x4d, 0x4d, 0xf4, 0x46, 0xd1, 0x3e, 0xfc, 0xd7, 0xc6, 0xf1, 0x7b,
                0x16, 0x7a, 0xac, 0x1a,
            ],
            [
                0xbd, 0x86, 0x16, 0x81, 0x1c, 0x6f, 0x5f, 0x76, 0x9e, 0xa4, 0x53, 0x9b, 0xba, 0xff,
                0x0f, 0x19, 0x8a, 0x6c, 0xdf, 0x3b, 0x28, 0x0d, 0xd4, 0x99, 0x26, 0x16, 0x3b, 0xd5,
                0x3f, 0x53, 0xa1, 0x21,
            ],
        ];
        let mut orchard_tree: CommitmentTree<MerkleHashOrchard, 32> = CommitmentTree::empty();
        for commitment in commitments {
            orchard_tree
                .append(MerkleHashOrchard::from_bytes(&commitment).unwrap())
                .unwrap();
        }
        // This value was produced by the Python test vector generation code implemented here:
        // https://github.com/zcash-hackworks/zcash-test-vectors/blob/f4d756410c8f2456f5d84cedf6dac6eb8c068eed/orchard_merkle_tree.py
        let anchor = [
            0xc8, 0x75, 0xbe, 0x2d, 0x60, 0x87, 0x3f, 0x8b, 0xcd, 0xeb, 0x91, 0x28, 0x2e, 0x64,
            0x2e, 0x0c, 0xc6, 0x5f, 0xf7, 0xd0, 0x64, 0x2d, 0x13, 0x7b, 0x28, 0xcf, 0x28, 0xcc,
            0x9c, 0x52, 0x7f, 0x0e,
        ];
        let anchor = orchard::Anchor::from(MerkleHashOrchard::from_bytes(&anchor).unwrap());
        assert_eq!(orchard::Anchor::from(orchard_tree.root()), anchor);
    }
}

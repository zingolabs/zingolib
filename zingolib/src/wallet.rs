//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
//! TODO: Add Mod Description Here

use error::{KeyError, WalletError};
use keys::unified::{UnifiedAddressId, UnifiedKeyStore};
use send::SendProgress;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::legacy::keys::NonHardenedChildIndex;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use pepper_sync::keys::transparent::{self, TransparentScope};
use pepper_sync::wallet::ShardTrees;
use pepper_sync::{
    keys::transparent::TransparentAddressId,
    wallet::{Locator, NullifierMap, OutputId, SyncState, WalletBlock, WalletTransaction},
};

use bip0039::Mnemonic;
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;
use std::time::SystemTime;

use crate::config::ChainType;

pub mod data;
pub mod error;
pub mod keys;
pub(crate) mod legacy;
pub mod traits;
pub mod utils;

//these mods contain pieces of the impl LightWallet
pub mod describe;
pub mod disk;
pub mod output;
pub mod propose;
pub mod send;
pub mod summary;
pub mod sync;
pub mod transaction;
mod zcb_traits;

/// TODO: Add Doc Comment Here!
// TODO: move to utils
pub fn now() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("should never fail when comparing with an instant so far in the past")
        .as_secs() as u32
}

/// Data used to initialize new instance of LightWallet
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
    /// Network type
    pub network: ChainType,
    /// The seed for the wallet, stored as a zip339 Mnemonic, and the account index.
    pub mnemonic: Option<Mnemonic>,
    /// The block height at which the wallet was created.
    pub birthday: BlockHeight,
    /// Unified key store
    pub unified_key_store: BTreeMap<zip32::AccountId, UnifiedKeyStore>,
    /// Unified_addresses
    pub unified_addresses: BTreeMap<UnifiedAddressId, UnifiedAddress>,
    /// Transparent addresses
    pub transparent_addresses: BTreeMap<TransparentAddressId, String>,
    /// Wallet blocks
    pub wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    /// Wallet transactions
    pub wallet_transactions: HashMap<TxId, WalletTransaction>,
    /// Nullifier map
    pub nullifier_map: NullifierMap,
    /// Outpoint map
    pub outpoint_map: BTreeMap<OutputId, Locator>,
    /// Shard trees
    pub shard_trees: ShardTrees,
    /// Sync state
    pub sync_state: SyncState,
    /// Progress of an outgoing transaction
    pub send_progress: SendProgress,
    /// Boolean for tracking whether the wallet state has changed since last save.
    pub save_required: bool,
    /// Wallet settings.
    pub wallet_settings: WalletSettings,
}

impl LightWallet {
    /// Create a new in-memory wallet.
    ///
    /// For wallets from fresh entropy, it is worth considering setting `birthday` to 100 blocks below current height
    /// of block chain to protect from re-orgs.
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
                    .into_iter()
                    .map(|account_index| {
                        Ok((
                            zip32::AccountId::try_from(account_index)?,
                            UnifiedKeyStore::new_from_mnemonic(&network, &mnemonic, account_index)?,
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

        let first_address_index = 0;
        let first_unified_address = unified_key_store
            .get(&zip32::AccountId::ZERO)
            .expect("key store always non-empty")
            .generate_unified_address(
                first_address_index,
                unified_key_store
                    .get(&zip32::AccountId::ZERO)
                    .expect("key store always non-empty")
                    .can_view(),
                false,
            )?;
        let mut unified_addresses = BTreeMap::new();
        unified_addresses.insert(
            UnifiedAddressId {
                account_id: zip32::AccountId::ZERO,
                address_index: first_address_index,
            },
            first_unified_address.clone(),
        );

        let mut transparent_addresses = BTreeMap::new();
        if let Some(transparent_address) = first_unified_address.transparent() {
            transparent_addresses.insert(
                TransparentAddressId::new(
                    zip32::AccountId::ZERO,
                    TransparentScope::External,
                    NonHardenedChildIndex::from_index(first_address_index).expect("infallible"),
                ),
                transparent::encode_address(&network, *transparent_address),
            );
        }

        Ok(Self {
            mnemonic,
            birthday: BlockHeight::from_u32(birthday.into()),
            unified_key_store,
            send_progress: SendProgress::new(0),
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: ShardTrees::new(),
            sync_state: SyncState::new(),
            transparent_addresses,
            unified_addresses,
            network,
            save_required: true,
            wallet_settings,
        })
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
    pub async fn save(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        if self.save_required {
            let network = self.network;
            let mut wallet_bytes: Vec<u8> = vec![];
            self.write(&mut wallet_bytes, &network).await?;
            self.save_required = false;
            Ok(Some(wallet_bytes))
        } else {
            Ok(None)
        }
    }

    /// Clears all wallet data obtained from the block chain including the sync state.
    ///
    /// Adds locators to the new sync state to prioritise scanning relevant parts of the chain on rescan.
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
                        .map(|height| (height, transaction.txid()))
                })
                .collect::<Vec<_>>(),
        );

        self.wallet_blocks.clear();
        self.wallet_transactions.clear();
        self.nullifier_map.clear();
        self.outpoint_map.clear();
        self.shard_trees = ShardTrees::new();

        self.save_required = true;
    }
}

/// Wallet settings.
#[derive(Debug, Clone)]
pub struct WalletSettings {
    /// Sync configuration.
    pub sync_config: pepper_sync::sync::SyncConfig,
}

#[cfg(test)]
mod tests {
    use incrementalmerkletree::frontier::CommitmentTree;
    use orchard::tree::MerkleHashOrchard;

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
                .unwrap()
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

//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
//! TODO: Add Mod Description Here

use append_only_vec::AppendOnlyVec;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use error::{KeyError, WalletError};
use keys::unified::UnifiedKeyStore;
use notes::query::OutputQuery;
use zcash_keys::{address::UnifiedAddress, keys::UnifiedFullViewingKey};
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::memo::Memo;

use log::{info, warn};
use rand::rngs::OsRng;
use rand::Rng;

use zingo_sync::{
    keys::transparent::TransparentAddressId,
    primitives::{Locator, NullifierMap, OutputId, SyncState, WalletBlock, WalletTransaction},
    witness::ShardTrees,
};

use bip0039::Mnemonic;
use std::collections::{BTreeMap, HashMap};
use std::{
    io::{self, Read, Write},
    sync::Arc,
    time::SystemTime,
};
use tokio::sync::RwLock;

use crate::config::ChainType;
use zcash_encoding::Optional;

use self::{data::WalletZecPriceInfo, message::Message};

pub mod data;
pub mod error;
pub mod keys;
pub(crate) mod message;
pub mod notes;
pub mod traits;
pub mod transaction_context;
pub mod transaction_record;
pub mod transaction_records_by_id;
pub mod tx_map;
pub mod utils;

//these mods contain pieces of the impl LightWallet
pub mod describe;
pub mod disk;
pub mod propose;
pub mod send;
pub mod sync;

pub(crate) use send::SendProgress;

/// TODO: Add Doc Comment Here!
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// TODO: Add Doc Comment Here!
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoDownloadOption {
    /// TODO: Add Doc Comment Here!
    NoMemos = 0,
    /// TODO: Add Doc Comment Here!
    WalletMemos,
    /// TODO: Add Doc Comment Here!
    AllMemos,
}

/// TODO: Add Doc Comment Here!
#[derive(Debug, Clone, Copy)]
pub struct WalletOptions {
    pub(crate) download_memos: MemoDownloadOption,
    /// TODO: Add Doc Comment Here!
    pub transaction_size_filter: Option<u32>,
}

/// TODO: Add Doc Comment Here!
pub const MAX_TRANSACTION_SIZE_DEFAULT: u32 = 500;

impl Default for WalletOptions {
    fn default() -> Self {
        WalletOptions {
            download_memos: MemoDownloadOption::WalletMemos,
            transaction_size_filter: Some(MAX_TRANSACTION_SIZE_DEFAULT),
        }
    }
}

impl WalletOptions {
    /// TODO: Add Doc Comment Here!
    pub const fn serialized_version() -> u64 {
        2
    }

    /// TODO: Add Doc Comment Here!
    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let external_version = reader.read_u64::<LittleEndian>()?;

        let download_memos = match reader.read_u8()? {
            0 => MemoDownloadOption::NoMemos,
            1 => MemoDownloadOption::WalletMemos,
            2 => MemoDownloadOption::AllMemos,
            v => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Bad download option {}", v),
                ));
            }
        };

        let transaction_size_filter = if external_version > 1 {
            Optional::read(reader, |mut r| r.read_u32::<LittleEndian>())?
        } else {
            Some(500)
        };

        Ok(Self {
            download_memos,
            transaction_size_filter,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        // Write the version
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        writer.write_u8(self.download_memos as u8)?;
        Optional::write(writer, self.transaction_size_filter, |mut w, filter| {
            w.write_u32::<LittleEndian>(filter)
        })
    }
}

/// Data used to initialize new instance of LightWallet
pub enum WalletBase {
    /// Generate a wallet with a new seed.
    FreshEntropy,
    /// Generate a wallet from a seed (account index = 0).
    SeedBytes([u8; 32]),
    /// Generate a wallet from a mnemonic phrase (account index = 0).
    MnemonicPhrase(String),
    /// Generate a wallet from a mnemonic (account index = 0).
    Mnemonic(Mnemonic),
    /// Generate a wallet from a seed and account index.
    SeedBytesAndAccount([u8; 32], u32),
    /// Generate a wallet from a mnemonic phrase and account index.
    MnemonicPhraseAndAccount(String, u32),
    /// Generate a wallet from a mnemonic and account index.
    MnemonicAndAccount(Mnemonic, u32),
    /// Generate a wallet from a unified full viewing key.
    Ufvk(String),
    /// Generate a wallet from a unified spending key.
    Usk(Vec<u8>),
}

impl WalletBase {
    /// TODO: Add Doc Comment Here!
    pub fn from_string(base: String) -> WalletBase {
        if (&base[0..5]) == "uview" {
            WalletBase::Ufvk(base)
        } else {
            WalletBase::MnemonicPhrase(base)
        }
    }
}

/// In-memory wallet data struct
pub struct LightWallet {
    /// The block height at which the wallet was created.
    ///
    /// As no relevant transactions related to this wallet will exist below the wallet's birthday, sync will start from
    /// this block height.
    pub birthday: BlockHeight,
    /// The seed for the wallet, stored as a zip339 Mnemonic, and the account index.
    /// Can be `None` in case of wallet without spending capability
    /// or created directly from spending keys.
    // TODO: we seem to support generating keys for a single account of choice which is stored here, this should be
    // reworked to support multiple accounts during sync integration
    mnemonic: Option<(Mnemonic, u32)>,
    /// Wallet options
    pub wallet_options: Arc<RwLock<WalletOptions>>, // TODO: revisit options
    /// Progress of an outgoing transaction
    send_progress: Arc<RwLock<SendProgress>>,
    /// The current price of ZEC. (time_fetched, price in USD)
    pub price: Arc<RwLock<WalletZecPriceInfo>>,
    /// Unified key store
    pub unified_key_store: UnifiedKeyStore,
    /// Wallet blocks
    pub wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    /// Wallet transactions
    pub wallet_transactions: HashMap<zcash_primitives::transaction::TxId, WalletTransaction>,
    /// Nullifier map
    pub nullifier_map: NullifierMap,
    /// Outpoint map
    pub outpoint_map: BTreeMap<OutputId, Locator>,
    /// Shard trees
    pub shard_trees: ShardTrees,
    /// Sync state
    pub sync_state: SyncState,
    /// Transparent addresses
    pub transparent_addresses: BTreeMap<TransparentAddressId, String>,
    /// Unified_addresses
    // TODO: sync integration, not yet integrated
    pub unified_addresses: append_only_vec::AppendOnlyVec<UnifiedAddress>,
    /// Network type
    pub network: ChainType,
}

impl LightWallet {
    /// Clears all the downloaded blocks and resets the state back to the initial block.
    /// After this, the wallet's initial state will need to be set
    /// and the wallet will need to be rescanned
    pub async fn clear_all(&mut self) {
        self.wallet_blocks.clear();
        self.wallet_transactions.clear();
        self.nullifier_map.sapling_mut().clear();
        self.nullifier_map.orchard_mut().clear();
        self.outpoint_map.clear();
        self.sync_state = SyncState::new();
    }

    ///TODO: Make this work for orchard too
    pub async fn decrypt_message(&self, enc: Vec<u8>) -> Result<Message, String> {
        let ufvk: UnifiedFullViewingKey = match (&self.unified_key_store).try_into() {
            Ok(ufvk) => ufvk,
            Err(e) => return Err(e.to_string()),
        };
        let sapling_ivk = if let Some(ivk) = ufvk.sapling() {
            ivk.to_external_ivk().prepare()
        } else {
            return Err(KeyError::NoViewCapability.to_string());
        };

        if let Ok(msg) = Message::decrypt(&enc, &sapling_ivk) {
            // If decryption succeeded for this IVK, return the decrypted memo and the matched address
            return Ok(msg);
        }

        Err("No message matched".to_string())
    }

    /// TODO: Add Doc Comment Here!
    pub fn memo_str(memo: Option<Memo>) -> Option<String> {
        match memo {
            Some(Memo::Text(m)) => Some(m.to_string()),
            Some(Memo::Arbitrary(_)) => Some("Wallet-internal memo".to_string()),
            _ => None,
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn new(
        network: ChainType,
        wallet_base: WalletBase,
        height: BlockHeight,
    ) -> Result<Self, WalletError> {
        let (unified_key_store, mnemonic) = match wallet_base {
            WalletBase::FreshEntropy => {
                let mut seed_bytes = [0u8; 32];
                // Create a random seed.
                let mut system_rng = OsRng;
                system_rng.fill(&mut seed_bytes);
                return Self::new(network, WalletBase::SeedBytes(seed_bytes), height);
            }
            WalletBase::SeedBytes(seed_bytes) => {
                return Self::new(
                    network,
                    WalletBase::SeedBytesAndAccount(seed_bytes, 0),
                    height,
                );
            }
            WalletBase::SeedBytesAndAccount(seed_bytes, account_index) => {
                let mnemonic = Mnemonic::from_entropy(seed_bytes)?;
                return Self::new(
                    network,
                    WalletBase::MnemonicAndAccount(mnemonic, account_index),
                    height,
                );
            }
            WalletBase::MnemonicPhrase(phrase) => {
                return Self::new(
                    network,
                    WalletBase::MnemonicPhraseAndAccount(phrase, 0),
                    height,
                );
            }
            WalletBase::MnemonicPhraseAndAccount(phrase, account_index) => {
                let mnemonic = Mnemonic::<bip0039::English>::from_phrase(phrase)?;
                return Self::new(
                    network,
                    WalletBase::MnemonicAndAccount(mnemonic, account_index),
                    height,
                );
            }
            WalletBase::Mnemonic(mnemonic) => {
                return Self::new(network, WalletBase::MnemonicAndAccount(mnemonic, 0), height);
            }
            WalletBase::MnemonicAndAccount(mnemonic, account_index) => {
                let unified_key_store =
                    UnifiedKeyStore::new_from_mnemonic(&network, &mnemonic, account_index)?;
                (unified_key_store, Some((mnemonic, account_index)))
            }
            WalletBase::Ufvk(ufvk_encoded) => {
                let unified_key_store = UnifiedKeyStore::new_from_ufvk(&network, ufvk_encoded)?;
                (unified_key_store, None)
            }
            WalletBase::Usk(unified_spending_key) => {
                let unified_key_store =
                    UnifiedKeyStore::new_from_usk(unified_spending_key.as_slice())?;
                (unified_key_store, None)
            }
        };

        let unified_addresses = AppendOnlyVec::new();
        unified_addresses.push(unified_key_store.generate_unified_address(
            0,
            unified_key_store.can_view(),
            false,
        )?);

        Ok(Self {
            mnemonic,
            wallet_options: Arc::new(RwLock::new(WalletOptions::default())),
            birthday: BlockHeight::from_u32(height.try_into().expect("should never overflow")),
            unified_key_store: UnifiedKeyStore::Empty, // TODO: not yet integrated
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            price: Arc::new(RwLock::new(WalletZecPriceInfo::default())),
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: zingo_sync::primitives::NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: zingo_sync::witness::ShardTrees::new(),
            sync_state: zingo_sync::primitives::SyncState::new(),
            transparent_addresses: BTreeMap::new(),
            unified_addresses: AppendOnlyVec::new(),
            network,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub async fn set_download_memo(&self, value: MemoDownloadOption) {
        self.wallet_options.write().await.download_memos = value;
    }

    /// TODO: Add Doc Comment Here!
    pub async fn set_latest_zec_price(&self, price: f64) {
        if price <= 0 as f64 {
            warn!("Tried to set a bad current zec price {}", price);
            return;
        }

        self.price.write().await.zec_price = Some((now(), price));
        info!("Set current ZEC Price to USD {}", price);
    }

    // Set the previous send's status as an error or success
    pub(super) async fn set_send_result(&self, result: Result<serde_json::Value, String>) {
        let mut p = self.send_progress.write().await;

        p.is_send_in_progress = false;
        p.last_result = Some(result);
    }

    /// Uses a query to select all notes across all transactions with specific properties and sum them
    pub fn sum_queried_output_values(&self, query: OutputQuery) -> u64 {
        self.wallet_transactions
            .values()
            .fold(0, |acc, transaction| {
                acc + {
                    let mut sum = 0;
                    let spend_status_query = query.spend_status;
                    if query.transparent() {
                        for output in transaction.transparent_coins().iter() {
                            if output.spend_status_query(spend_status_query) {
                                sum += output.value()
                            }
                        }
                    }
                    if query.sapling() {
                        for output in transaction.sapling_notes().iter() {
                            if output.spend_status_query(spend_status_query) {
                                sum += output.value()
                            }
                        }
                    }
                    if query.orchard() {
                        for output in transaction.orchard_notes().iter() {
                            if output.spend_status_query(spend_status_query) {
                                sum += output.value()
                            }
                        }
                    }
                    sum
                }
            })
    }
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

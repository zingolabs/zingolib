//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
//! TODO: Add Mod Description Here

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use error::KeyError;
use getset::{Getters, MutGetters};
use zcash_keys::keys::UnifiedFullViewingKey;
#[cfg(feature = "sync")]
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::memo::Memo;

use log::{info, warn};
use rand::rngs::OsRng;
use rand::Rng;

#[cfg(feature = "sync")]
use zingo_sync::{
    primitives::{NullifierMap, SyncState, WalletBlock, WalletTransaction},
    witness::ShardTrees,
};

use bip0039::Mnemonic;
#[cfg(feature = "sync")]
use std::collections::BTreeMap;
use std::{
    cmp,
    collections::HashMap,
    io::{self, Error, ErrorKind, Read, Write},
    sync::{atomic::AtomicU64, Arc},
    time::SystemTime,
};
use tokio::sync::RwLock;

use crate::config::ZingoConfig;
use zcash_client_backend::proto::service::TreeState;
use zcash_encoding::Optional;

use self::keys::unified::WalletCapability;

use self::{
    data::{BlockData, WalletZecPriceInfo},
    message::Message,
    transaction_context::TransactionContext,
    tx_map::TxMap,
};

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
pub mod witnesses;

#[cfg(feature = "sync")]
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
    /// TODO: Add Doc Comment Here!
    FreshEntropy,
    /// TODO: Add Doc Comment Here!
    SeedBytes([u8; 32]),
    /// TODO: Add Doc Comment Here!
    MnemonicPhrase(String),
    /// TODO: Add Doc Comment Here!
    Mnemonic(Mnemonic),
    /// TODO: Add Doc Comment Here!
    SeedBytesAndIndex([u8; 32], u32),
    /// TODO: Add Doc Comment Here!
    MnemonicPhraseAndIndex(String, u32),
    /// TODO: Add Doc Comment Here!
    MnemonicAndIndex(Mnemonic, u32),
    /// Unified full viewing key
    Ufvk(String),
    /// Unified spending key
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
#[derive(Getters, MutGetters)]
pub struct LightWallet {
    // The block at which this wallet was born. Rescans
    // will start from here.
    birthday: AtomicU64,

    /// The seed for the wallet, stored as a zip339 Mnemonic, and the account index.
    /// Can be `None` in case of wallet without spending capability
    /// or created directly from spending keys.
    mnemonic: Option<(Mnemonic, u32)>,

    /// The last 100 blocks, used if something gets re-orged
    pub last_100_blocks: Arc<RwLock<Vec<BlockData>>>,

    /// Wallet options
    pub wallet_options: Arc<RwLock<WalletOptions>>,

    /// Highest verified block
    pub(crate) verified_tree: Arc<RwLock<Option<TreeState>>>,

    /// Progress of an outgoing transaction
    send_progress: Arc<RwLock<SendProgress>>,

    /// The current price of ZEC. (time_fetched, price in USD)
    pub price: Arc<RwLock<WalletZecPriceInfo>>,

    /// Local state needed to submit (compact)block-requests to the proxy
    /// and interpret responses
    pub transaction_context: TransactionContext,

    /// Wallet compact blocks
    #[cfg(feature = "sync")]
    #[getset(get = "pub", get_mut = "pub")]
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,

    /// Wallet transactions
    #[cfg(feature = "sync")]
    #[getset(get = "pub", get_mut = "pub")]
    wallet_transactions: HashMap<zcash_primitives::transaction::TxId, WalletTransaction>,

    /// Nullifier map
    #[cfg(feature = "sync")]
    #[getset(get = "pub", get_mut = "pub")]
    nullifier_map: NullifierMap,

    /// Shard trees
    #[cfg(feature = "sync")]
    #[getset(get = "pub", get_mut = "pub")]
    shard_trees: ShardTrees,

    /// Sync state
    #[cfg(feature = "sync")]
    #[getset(get = "pub", get_mut = "pub")]
    sync_state: SyncState,
}

impl LightWallet {
    // This function will likely be used if/when we reimplement key import
    #[allow(dead_code)]
    fn adjust_wallet_birthday(&self, new_birthday: u64) {
        let mut wallet_birthday = self.birthday.load(std::sync::atomic::Ordering::SeqCst);
        if new_birthday < wallet_birthday {
            wallet_birthday = cmp::max(
                new_birthday,
                self.transaction_context.config.sapling_activation_height(),
            );
            self.birthday
                .store(wallet_birthday, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Clears all the downloaded blocks and resets the state back to the initial block.
    /// After this, the wallet's initial state will need to be set
    /// and the wallet will need to be rescanned
    pub async fn clear_all(&self) {
        self.last_100_blocks.write().await.clear();
        self.transaction_context
            .transaction_metadata_set
            .write()
            .await
            .clear();
    }

    ///TODO: Make this work for orchard too
    pub async fn decrypt_message(&self, enc: Vec<u8>) -> Result<Message, String> {
        let ufvk: UnifiedFullViewingKey =
            match self.wallet_capability().unified_key_store().try_into() {
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
    pub fn new(config: ZingoConfig, base: WalletBase, height: u64) -> io::Result<Self> {
        let (wc, mnemonic) = match base {
            WalletBase::FreshEntropy => {
                let mut seed_bytes = [0u8; 32];
                // Create a random seed.
                let mut system_rng = OsRng;
                system_rng.fill(&mut seed_bytes);
                return Self::new(config, WalletBase::SeedBytes(seed_bytes), height);
            }
            WalletBase::SeedBytes(seed_bytes) => {
                return Self::new(config, WalletBase::SeedBytesAndIndex(seed_bytes, 0), height);
            }
            WalletBase::SeedBytesAndIndex(seed_bytes, position) => {
                let mnemonic = Mnemonic::from_entropy(seed_bytes).map_err(|e| {
                    Error::new(
                        ErrorKind::InvalidData,
                        format!("Error parsing phrase: {}", e),
                    )
                })?;
                return Self::new(
                    config,
                    WalletBase::MnemonicAndIndex(mnemonic, position),
                    height,
                );
            }
            WalletBase::MnemonicPhrase(phrase) => {
                return Self::new(
                    config,
                    WalletBase::MnemonicPhraseAndIndex(phrase, 0),
                    height,
                );
            }
            WalletBase::MnemonicPhraseAndIndex(phrase, position) => {
                let mnemonic = Mnemonic::<bip0039::English>::from_phrase(phrase)
                    .and_then(|m| Mnemonic::from_entropy(m.entropy()))
                    .map_err(|e| {
                        Error::new(
                            ErrorKind::InvalidData,
                            format!("Error parsing phrase: {}", e),
                        )
                    })?;
                // Notice that `.and_then(|m| Mnemonic::from_entropy(m.entropy()))`
                // should be a no-op, but seems to be needed on android for some reason
                // TODO: Test the this cfg actually works
                //#[cfg(target_os = "android")]
                return Self::new(
                    config,
                    WalletBase::MnemonicAndIndex(mnemonic, position),
                    height,
                );
            }
            WalletBase::Mnemonic(mnemonic) => {
                return Self::new(config, WalletBase::MnemonicAndIndex(mnemonic, 0), height);
            }
            WalletBase::MnemonicAndIndex(mnemonic, position) => {
                let wc = WalletCapability::new_from_phrase(&config, &mnemonic, position)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
                (wc, Some((mnemonic, position)))
            }
            WalletBase::Ufvk(ufvk_encoded) => {
                let wc = WalletCapability::new_from_ufvk(&config, ufvk_encoded).map_err(|e| {
                    Error::new(ErrorKind::InvalidData, format!("Error parsing UFVK: {}", e))
                })?;
                (wc, None)
            }
            WalletBase::Usk(unified_spending_key) => {
                let wc = WalletCapability::new_from_usk(unified_spending_key.as_slice()).map_err(
                    |e| {
                        Error::new(
                            ErrorKind::InvalidData,
                            format!("Error parsing unified spending key: {}", e),
                        )
                    },
                )?;
                (wc, None)
            }
        };

        if let Err(e) = wc.new_address(wc.can_view(), false) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("could not create initial address: {e}"),
            ));
        };
        let transaction_metadata_set = if wc.unified_key_store().is_spending_key() {
            Arc::new(RwLock::new(TxMap::new_with_witness_trees(
                wc.transparent_child_addresses().clone(),
                wc.get_rejection_addresses().clone(),
                wc.rejection_ivk().map_err(|e| {
                    Error::new(
                        ErrorKind::InvalidData,
                        format!("Error with transparent key: {e}"),
                    )
                })?,
            )))
        } else {
            Arc::new(RwLock::new(TxMap::new_treeless(
                wc.transparent_child_addresses().clone(),
            )))
        };
        let transaction_context =
            TransactionContext::new(&config, Arc::new(wc), transaction_metadata_set);
        Ok(Self {
            last_100_blocks: Arc::new(RwLock::new(vec![])),
            mnemonic,
            wallet_options: Arc::new(RwLock::new(WalletOptions::default())),
            birthday: AtomicU64::new(height),
            verified_tree: Arc::new(RwLock::new(None)),
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            price: Arc::new(RwLock::new(WalletZecPriceInfo::default())),
            transaction_context,
            #[cfg(feature = "sync")]
            wallet_blocks: BTreeMap::new(),
            #[cfg(feature = "sync")]
            wallet_transactions: HashMap::new(),
            #[cfg(feature = "sync")]
            nullifier_map: zingo_sync::primitives::NullifierMap::new(),
            #[cfg(feature = "sync")]
            shard_trees: zingo_sync::witness::ShardTrees::new(),
            #[cfg(feature = "sync")]
            sync_state: zingo_sync::primitives::SyncState::new(),
        })
    }

    /// TODO: Add Doc Comment Here!
    pub async fn set_blocks(&self, new_blocks: Vec<BlockData>) {
        let mut blocks = self.last_100_blocks.write().await;
        blocks.clear();
        blocks.extend_from_slice(&new_blocks[..]);
    }

    /// TODO: Add Doc Comment Here!
    pub async fn set_download_memo(&self, value: MemoDownloadOption) {
        self.wallet_options.write().await.download_memos = value;
    }

    /// TODO: Add Doc Comment Here!
    pub async fn set_initial_block(&self, height: u64, hash: &str, _sapling_tree: &str) -> bool {
        let mut blocks = self.last_100_blocks.write().await;
        if !blocks.is_empty() {
            return false;
        }

        blocks.push(BlockData::new_with(height, &hex::decode(hash).unwrap()));

        true
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

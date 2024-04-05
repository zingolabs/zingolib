//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use json::JsonValue;
use log::{error, info, warn};
use orchard::keys::SpendingKey as OrchardSpendingKey;
use orchard::note_encryption::OrchardDomain;
use orchard::tree::MerkleHashOrchard;
use rand::rngs::OsRng;
use rand::Rng;
use sapling_crypto::note_encryption::SaplingDomain;

use sapling_crypto::zip32::DiversifiableFullViewingKey;
use shardtree::error::ShardTreeError;
use shardtree::store::memory::MemoryShardStore;
use shardtree::ShardTree;
use std::convert::Infallible;
use std::ops::Add;
use std::{
    cmp,
    io::{self, Error, ErrorKind, Read, Write},
    sync::{atomic::AtomicU64, Arc},
    time::SystemTime,
};
use tokio::sync::RwLock;
use zcash_primitives::zip339::Mnemonic;

use zcash_client_backend::proto::service::TreeState;
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::Domain;

use zcash_primitives::transaction::{self};
use zcash_primitives::{consensus::BlockHeight, memo::Memo, transaction::components::Amount};

use zingo_status::confirmation_status::ConfirmationStatus;
use zingoconfig::ZingoConfig;

use crate::wallet::data::TransactionRecord;
use crate::wallet::notes::NoteInterface;
use crate::wallet::notes::ShieldedNoteInterface;

use crate::wallet::traits::{Diversifiable as _, ReadableWriteable};

use self::data::{WitnessTrees, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL};
use self::keys::unified::Fvk as _;
use self::keys::unified::{Capability, WalletCapability};
use self::traits::Recipient;
use self::traits::{DomainWalletExt, SpendableNote};

use self::{
    data::{BlockData, WalletZecPriceInfo},
    message::Message,
    transaction_context::TransactionContext,
    transactions::TxMapAndMaybeTrees,
};

pub mod data;
pub mod keys;
pub(crate) mod message;
pub mod notes;
pub mod traits;
pub mod transaction_context;
pub mod transaction_record;
pub(crate) mod transactions;
pub mod utils;

//these mods contain pieces of the impl LightWallet

pub mod spend_wallet;

pub mod send;

pub mod disk;

pub(crate) use send::SendProgress;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    Orchard,
    Sapling,
    Transparent,
}

impl From<Pool> for JsonValue {
    fn from(value: Pool) -> Self {
        match value {
            Pool::Orchard => JsonValue::String(String::from("Orchard")),
            Pool::Sapling => JsonValue::String(String::from("Sapling")),
            Pool::Transparent => JsonValue::String(String::from("Transparent")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoDownloadOption {
    NoMemos = 0,
    WalletMemos,
    AllMemos,
}

#[derive(Debug, Clone, Copy)]
pub struct WalletOptions {
    pub(crate) download_memos: MemoDownloadOption,
    pub transaction_size_filter: Option<u32>,
}

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
    pub const fn serialized_version() -> u64 {
        2
    }

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
    FreshEntropy,
    SeedBytes([u8; 32]),
    MnemonicPhrase(String),
    Mnemonic(Mnemonic),
    SeedBytesAndIndex([u8; 32], u32),
    MnemonicPhraseAndIndex(String, u32),
    MnemonicAndIndex(Mnemonic, u32),
    /// Unified full viewing key
    Ufvk(String),
    /// Unified spending key
    Usk(Vec<u8>),
}
impl WalletBase {
    pub fn from_string(base: String) -> WalletBase {
        if (&base[0..5]) == "uview" {
            WalletBase::Ufvk(base)
        } else {
            WalletBase::MnemonicPhrase(base)
        }
    }
}

pub struct LightWallet {
    // The block at which this wallet was born. Rescans
    // will start from here.
    birthday: AtomicU64,

    /// The seed for the wallet, stored as a zip339 Mnemonic, and the account index.
    /// Can be `None` in case of wallet without spending capability
    /// or created directly from spending keys.
    mnemonic: Option<(Mnemonic, u32)>,

    // The last 100 blocks, used if something gets re-orged
    pub blocks: Arc<RwLock<Vec<BlockData>>>,

    // Wallet options
    pub wallet_options: Arc<RwLock<WalletOptions>>,

    // Highest verified block
    pub(crate) verified_tree: Arc<RwLock<Option<TreeState>>>,

    // Progress of an outgoing transaction
    send_progress: Arc<RwLock<SendProgress>>,

    // The current price of ZEC. (time_fetched, price in USD)
    pub price: Arc<RwLock<WalletZecPriceInfo>>,

    // Local state needed to submit [compact]block-requests to the proxy
    // and interpret responses
    pub transaction_context: TransactionContext,
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
        self.blocks.write().await.clear();
        self.transaction_context
            .transaction_metadata_set
            .write()
            .await
            .clear();
    }

    ///TODO: Make this work for orchard too
    pub async fn decrypt_message(&self, enc: Vec<u8>) -> Result<Message, String> {
        let sapling_ivk = DiversifiableFullViewingKey::try_from(&*self.wallet_capability())?
            .derive_ivk::<keys::unified::External>();

        if let Ok(msg) = Message::decrypt(&enc, &sapling_ivk.ivk) {
            // If decryption succeeded for this IVK, return the decrypted memo and the matched address
            return Ok(msg);
        }

        Err("No message matched".to_string())
    }

    pub fn memo_str(memo: Option<Memo>) -> Option<String> {
        match memo {
            Some(Memo::Text(m)) => Some(m.to_string()),
            Some(Memo::Arbitrary(_)) => Some("Wallet-internal memo".to_string()),
            _ => None,
        }
    }

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
                let mnemonic = Mnemonic::from_phrase(phrase)
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

        if let Err(e) = wc.new_address(wc.can_view()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("could not create initial address: {e}"),
            ));
        };
        let transaction_metadata_set = if wc.can_spend_from_all_pools() {
            Arc::new(RwLock::new(TxMapAndMaybeTrees::new_with_witness_trees()))
        } else {
            Arc::new(RwLock::new(TxMapAndMaybeTrees::new_treeless()))
        };
        let transaction_context =
            TransactionContext::new(&config, Arc::new(wc), transaction_metadata_set);
        Ok(Self {
            blocks: Arc::new(RwLock::new(vec![])),
            mnemonic,
            wallet_options: Arc::new(RwLock::new(WalletOptions::default())),
            birthday: AtomicU64::new(height),
            verified_tree: Arc::new(RwLock::new(None)),
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            price: Arc::new(RwLock::new(WalletZecPriceInfo::default())),
            transaction_context,
        })
    }

    pub async fn set_blocks(&self, new_blocks: Vec<BlockData>) {
        let mut blocks = self.blocks.write().await;
        blocks.clear();
        blocks.extend_from_slice(&new_blocks[..]);
    }

    pub async fn set_download_memo(&self, value: MemoDownloadOption) {
        self.wallet_options.write().await.download_memos = value;
    }

    pub async fn set_initial_block(&self, height: u64, hash: &str, _sapling_tree: &str) -> bool {
        let mut blocks = self.blocks.write().await;
        if !blocks.is_empty() {
            return false;
        }

        blocks.push(BlockData::new_with(height, &hex::decode(hash).unwrap()));

        true
    }

    pub async fn set_latest_zec_price(&self, price: f64) {
        if price <= 0 as f64 {
            warn!("Tried to set a bad current zec price {}", price);
            return;
        }

        self.price.write().await.zec_price = Some((now(), price));
        info!("Set current ZEC Price to USD {}", price);
    }

    // Set the previous send's status as an error
    async fn set_send_error(&self, e: String) {
        let mut p = self.send_progress.write().await;

        p.is_send_in_progress = false;
        p.last_error = Some(e);
    }

    // Set the previous send's status as success
    async fn set_send_success(&self, transaction_id: String) {
        let mut p = self.send_progress.write().await;

        p.is_send_in_progress = false;
        p.last_transaction_id = Some(transaction_id);
    }
}

pub mod describe {
    use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

    use json::JsonValue;
    use log::{error, info, warn};
    use orchard::keys::SpendingKey as OrchardSpendingKey;
    use orchard::note_encryption::OrchardDomain;
    use orchard::tree::MerkleHashOrchard;
    use rand::rngs::OsRng;
    use rand::Rng;
    use sapling_crypto::note_encryption::SaplingDomain;

    use sapling_crypto::zip32::DiversifiableFullViewingKey;
    use shardtree::error::ShardTreeError;
    use shardtree::store::memory::MemoryShardStore;
    use shardtree::ShardTree;
    use std::convert::Infallible;
    use std::ops::Add;
    use std::{
        cmp,
        io::{self, Error, ErrorKind, Read, Write},
        sync::{atomic::AtomicU64, Arc},
        time::SystemTime,
    };
    use tokio::sync::RwLock;
    use zcash_primitives::zip339::Mnemonic;

    use zcash_client_backend::proto::service::TreeState;
    use zcash_encoding::{Optional, Vector};
    use zcash_note_encryption::Domain;

    use zcash_primitives::transaction::{self};
    use zcash_primitives::{consensus::BlockHeight, memo::Memo, transaction::components::Amount};

    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingoconfig::ZingoConfig;

    use crate::wallet::data::TransactionRecord;
    use crate::wallet::notes::NoteInterface;
    use crate::wallet::notes::ShieldedNoteInterface;

    use crate::wallet::traits::{Diversifiable as _, ReadableWriteable};

    use super::data::{WitnessTrees, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL};
    use super::keys::unified::Fvk as _;
    use super::keys::unified::{Capability, WalletCapability};
    use super::notes::TransparentNote;
    use super::traits::Recipient;
    use super::traits::{DomainWalletExt, SpendableNote};

    use super::{
        data::{BlockData, WalletZecPriceInfo},
        message::Message,
        transaction_context::TransactionContext,
        transactions::TxMapAndMaybeTrees,
    };

    use super::LightWallet;
    impl LightWallet {
        #[allow(clippy::type_complexity)]
        pub async fn shielded_balance<D>(
            &self,
            target_addr: Option<String>,
            filters: &[Box<dyn Fn(&&D::WalletNote, &TransactionRecord) -> bool + '_>],
        ) -> Option<u64>
        where
            D: DomainWalletExt,
            <D as Domain>::Note: PartialEq + Clone,
            <D as Domain>::Recipient: Recipient,
        {
            let fvk = D::wc_to_fvk(&self.wallet_capability()).ok()?;
            let filter_notes_by_target_addr = |notedata: &&D::WalletNote| match target_addr.as_ref()
            {
                Some(addr) => {
                    let diversified_address =
                        &fvk.diversified_address(*notedata.diversifier()).unwrap();
                    *addr
                        == diversified_address
                            .b32encode_for_network(&self.transaction_context.config.chain)
                }
                None => true, // If the addr is none, then get all addrs.
            };
            Some(
                self.transaction_context
                    .transaction_metadata_set
                    .read()
                    .await
                    .current
                    .values()
                    .map(|transaction| {
                        let mut filtered_notes: Box<dyn Iterator<Item = &D::WalletNote>> = Box::new(
                            D::WalletNote::transaction_metadata_notes(transaction)
                                .iter()
                                .filter(filter_notes_by_target_addr),
                        );
                        // All filters in iterator are applied, by this loop
                        for filtering_fn in filters {
                            filtered_notes = Box::new(
                                filtered_notes.filter(|nnmd| filtering_fn(nnmd, transaction)),
                            )
                        }
                        filtered_notes
                            .map(|notedata| {
                                if notedata.spent().is_none() && notedata.pending_spent().is_none()
                                {
                                    <D::WalletNote as ShieldedNoteInterface>::value(notedata)
                                } else {
                                    0
                                }
                            })
                            .sum::<u64>()
                    })
                    .sum::<u64>(),
            )
        }

        pub async fn spendable_orchard_balance(&self, target_addr: Option<String>) -> Option<u64> {
            if let Capability::Spend(_) = self.wallet_capability().orchard {
                self.verified_balance::<OrchardDomain>(target_addr).await
            } else {
                None
            }
        }

        pub async fn spendable_sapling_balance(&self, target_addr: Option<String>) -> Option<u64> {
            if let Capability::Spend(_) = self.wallet_capability().sapling {
                self.verified_balance::<SaplingDomain>(target_addr).await
            } else {
                None
            }
        }

        pub async fn tbalance(&self, addr: Option<String>) -> Option<u64> {
            if self.wallet_capability().transparent.can_view() {
                Some(
                    self.get_utxos()
                        .await
                        .iter()
                        .filter(|utxo| match addr.as_ref() {
                            Some(a) => utxo.address == *a,
                            None => true,
                        })
                        .map(|utxo| utxo.value)
                        .sum::<u64>(),
                )
            } else {
                None
            }
        }

        pub fn transactions(&self) -> Arc<RwLock<TxMapAndMaybeTrees>> {
            self.transaction_context.transaction_metadata_set.clone()
        }

        pub async fn unverified_balance<D: DomainWalletExt>(
            &self,
            target_addr: Option<String>,
        ) -> Option<u64>
        where
            <D as Domain>::Recipient: Recipient,
            <D as Domain>::Note: PartialEq + Clone,
        {
            let anchor_height = self.get_anchor_height().await;
            #[allow(clippy::type_complexity)]
            let filters: &[Box<dyn Fn(&&D::WalletNote, &TransactionRecord) -> bool>] =
                &[Box::new(|nnmd, transaction| {
                    !transaction
                        .status
                        .is_confirmed_before_or_at(&BlockHeight::from_u32(anchor_height))
                        || nnmd.pending_receipt()
                })];
            self.shielded_balance::<D>(target_addr, filters).await
        }

        pub async fn verified_balance<D: DomainWalletExt>(
            &self,
            target_addr: Option<String>,
        ) -> Option<u64>
        where
            <D as Domain>::Recipient: Recipient,
            <D as Domain>::Note: PartialEq + Clone,
        {
            let anchor_height = self.get_anchor_height().await;
            #[allow(clippy::type_complexity)]
            let filters: &[Box<dyn Fn(&&D::WalletNote, &TransactionRecord) -> bool>] = &[
                Box::new(|_, transaction| {
                    transaction
                        .status
                        .is_confirmed_before_or_at(&BlockHeight::from_u32(anchor_height))
                }),
                Box::new(|nnmd, _| !nnmd.pending_receipt()),
            ];
            self.shielded_balance::<D>(target_addr, filters).await
        }

        pub fn wallet_capability(&self) -> Arc<WalletCapability> {
            self.transaction_context.key.clone()
        }

        pub(crate) fn note_address<D: DomainWalletExt>(
            network: &zingoconfig::ChainType,
            note: &D::WalletNote,
            wallet_capability: &WalletCapability,
        ) -> String
        where
            <D as Domain>::Recipient: Recipient,
            <D as Domain>::Note: PartialEq + Clone,
        {
            D::wc_to_fvk(wallet_capability).expect("to get fvk from wc")
            .diversified_address(*note.diversifier())
            .and_then(|address| {
                D::ua_from_contained_receiver(wallet_capability, &address)
                    .map(|ua| ua.encode(network))
            })
            .unwrap_or("Diversifier not in wallet. Perhaps you restored from seed and didn't restore addresses".to_string())
        }
        pub fn mnemonic(&self) -> Option<&(Mnemonic, u32)> {
            self.mnemonic.as_ref()
        }

        /// Get the height of the anchor block
        pub async fn get_anchor_height(&self) -> u32 {
            match self.get_target_height_and_anchor_offset().await {
                Some((height, anchor_offset)) => height - anchor_offset as u32 - 1,
                None => 0,
            }
        }

        pub async fn get_birthday(&self) -> u64 {
            let birthday = self.birthday.load(std::sync::atomic::Ordering::SeqCst);
            if birthday == 0 {
                self.get_first_transaction_block().await
            } else {
                cmp::min(self.get_first_transaction_block().await, birthday)
            }
        }

        /// Return a copy of the blocks currently in the wallet, needed to process possible reorgs
        pub async fn get_blocks(&self) -> Vec<BlockData> {
            self.blocks.read().await.iter().cloned().collect()
        }

        // Get the first block that this wallet has a transaction in. This is often used as the wallet's "birthday"
        // If there are no transactions, then the actual birthday (which is recorder at wallet creation) is returned
        // If no birthday was recorded, return the sapling activation height
        pub async fn get_first_transaction_block(&self) -> u64 {
            // Find the first transaction
            let earliest_block = self
                .transaction_context
                .transaction_metadata_set
                .read()
                .await
                .current
                .values()
                .map(|wtx| u64::from(wtx.status.get_height()))
                .min();

            let birthday = self.birthday.load(std::sync::atomic::Ordering::SeqCst);
            earliest_block // Returns optional, so if there's no transactions, it'll get the activation height
                .unwrap_or(cmp::max(
                    birthday,
                    self.transaction_context.config.sapling_activation_height(),
                ))
        }

        // Get all (unspent) utxos. Unconfirmed spent utxos are included
        pub async fn get_utxos(&self) -> Vec<TransparentNote> {
            self.transaction_context
                .transaction_metadata_set
                .read()
                .await
                .current
                .values()
                .flat_map(|transaction| {
                    transaction
                        .transparent_notes
                        .iter()
                        .filter(|utxo| !utxo.is_spent())
                })
                .cloned()
                .collect::<Vec<TransparentNote>>()
        }

        pub async fn last_synced_hash(&self) -> String {
            self.blocks
                .read()
                .await
                .first()
                .map(|block| block.hash())
                .unwrap_or_default()
        }

        /// TODO: How do we know that 'sapling_activation_height - 1' is only returned
        /// when it should be?  When should it be?
        pub async fn last_synced_height(&self) -> u64 {
            self.blocks
                .read()
                .await
                .first()
                .map(|block| block.height)
                .unwrap_or(self.transaction_context.config.sapling_activation_height() - 1)
        }

        pub async fn maybe_verified_orchard_balance(&self, addr: Option<String>) -> Option<u64> {
            self.shielded_balance::<OrchardDomain>(addr, &[]).await
        }

        pub async fn maybe_verified_sapling_balance(&self, addr: Option<String>) -> Option<u64> {
            self.shielded_balance::<SaplingDomain>(addr, &[]).await
        }
    }
}

//This function will likely be used again if/when we re-implement key import
#[allow(dead_code)]
fn decode_orchard_spending_key(
    expected_hrp: &str,
    s: &str,
) -> Result<Option<OrchardSpendingKey>, String> {
    match bech32::decode(s) {
        Ok((hrp, bytes, variant)) => {
            use bech32::FromBase32;
            if hrp != expected_hrp {
                return Err(format!(
                    "invalid human-readable-part {hrp}, expected {expected_hrp}.",
                ));
            }
            if variant != bech32::Variant::Bech32m {
                return Err("Wrong encoding, expected bech32m".to_string());
            }
            match Vec::<u8>::from_base32(&bytes).map(<[u8; 32]>::try_from) {
                Ok(Ok(b)) => Ok(OrchardSpendingKey::from_bytes(b).into()),
                Ok(Err(e)) => Err(format!("key {s} decodes to {e:?}, which is not 32 bytes")),
                Err(e) => Err(e.to_string()),
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod test {
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

//! In all cases in this file "external_version" refers to a serialization version that is interpreted
//! from a source outside of the code-base e.g. a wallet-file.
use crate::blaze::fetch_full_transaction::TransactionContext;
use crate::compact_formats::TreeState;
use crate::wallet::data::TransactionMetadata;
use crate::wallet_internal_memo_handling::create_wallet_internal_memo_version_0;

use crate::wallet::data::SpendableSaplingNote;
use bip0039::Mnemonic;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use futures::Future;
use log::{error, info, warn};
use orchard::keys::SpendingKey as OrchardSpendingKey;
use orchard::note_encryption::OrchardDomain;
use orchard::tree::MerkleHashOrchard;
use orchard::Anchor;
use rand::rngs::OsRng;
use rand::Rng;
use std::{
    cmp,
    collections::HashMap,
    io::{self, Error, ErrorKind, Read, Write},
    sync::{atomic::AtomicU64, mpsc::channel, Arc},
    time::SystemTime,
};
use tokio::sync::RwLock;
use zcash_client_backend::address;
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::Domain;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::merkle_tree::CommitmentTree;
use zcash_primitives::sapling::note_encryption::SaplingDomain;
use zcash_primitives::sapling::SaplingIvk;
use zcash_primitives::transaction::builder::Progress;
use zcash_primitives::{
    consensus::BlockHeight,
    legacy::Script,
    memo::Memo,
    sapling::prover::TxProver,
    transaction::{
        builder::Builder,
        components::{amount::DEFAULT_FEE, Amount, OutPoint, TxOut},
    },
};

use self::data::SpendableOrchardNote;
use self::keys::unified::ReceiverSelection;
use self::keys::unified::UnifiedSpendCapability;
use self::traits::Recipient;
use self::traits::{DomainWalletExt, ReceivedNoteAndMetadata, SpendableNote};
use self::{
    data::{
        BlockData, ReceivedOrchardNoteAndMetadata, ReceivedSaplingNoteAndMetadata, Utxo,
        WalletZecPriceInfo,
    },
    message::Message,
    transactions::TransactionMetadataSet,
};
use zingoconfig::ZingoConfig;

pub(crate) mod data;
pub(crate) mod keys;
pub(crate) mod message;
pub(crate) mod traits;
pub(crate) mod transactions;
pub(crate) mod utils;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[derive(Debug, Clone)]
pub struct SendProgress {
    pub id: u32,
    pub is_send_in_progress: bool,
    pub progress: u32,
    pub total: u32,
    pub last_error: Option<String>,
    pub last_transaction_id: Option<String>,
}

impl SendProgress {
    fn new(id: u32) -> Self {
        SendProgress {
            id,
            is_send_in_progress: false,
            progress: 0,
            total: 0,
            last_error: None,
            last_transaction_id: None,
        }
    }
}

// Enum to refer to the first or last position of the Node
pub enum NodePosition {
    Oldest,
    Highest,
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
    pub(crate) transaction_size_filter: Option<u32>,
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
    pub fn serialized_version() -> u64 {
        return 2;
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

pub struct LightWallet {
    // The block at which this wallet was born. Rescans
    // will start from here.
    birthday: AtomicU64,

    /// The seed for the wallet, stored as a bip0039 Mnemonic
    mnemonic: Mnemonic,

    // The last 100 blocks, used if something gets re-orged
    pub(super) blocks: Arc<RwLock<Vec<BlockData>>>,

    // Wallet options
    pub(crate) wallet_options: Arc<RwLock<WalletOptions>>,

    // Heighest verified block
    pub(crate) verified_tree: Arc<RwLock<Option<TreeState>>>,

    // Progress of an outgoing transaction
    send_progress: Arc<RwLock<SendProgress>>,

    // The current price of ZEC. (time_fetched, price in USD)
    pub price: Arc<RwLock<WalletZecPriceInfo>>,

    // Local state needed to submit [compact]block-requests to the proxy
    // and interpret responses
    pub(crate) transaction_context: TransactionContext,
}

use crate::wallet::traits::{Diversifiable as _, ReadableWriteable};
impl LightWallet {
    pub fn serialized_version() -> u64 {
        return 26;
    }

    pub fn new(config: ZingoConfig, seed_phrase: Option<String>, height: u64) -> io::Result<Self> {
        let mnemonic = if seed_phrase.is_none() {
            let mut seed_bytes = [0u8; 32];
            // Create a random seed.
            let mut system_rng = OsRng;
            system_rng.fill(&mut seed_bytes);
            Mnemonic::from_entropy(seed_bytes)
        } else {
            let mnemonic = Mnemonic::from_phrase(seed_phrase.unwrap().as_str());

            // This should be a no-op, but seems to be needed on android for some reason
            // TODO: Test the this cfg actually works
            //#[cfg(target_os = "android")]

            let mnemonic = mnemonic.and_then(|m| Mnemonic::from_entropy(m.entropy()));

            mnemonic
        }
        .map_err(|e| {
            let e = format!("Error parsing phrase: {}", e);
            //error!("{}", e);
            Error::new(ErrorKind::InvalidData, e)
        })?;
        let mut usc = UnifiedSpendCapability::new_from_phrase(&config, &mnemonic, 0)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
        usc.new_address(ReceiverSelection {
            sapling: true,
            orchard: true,
            transparent: true,
        })
        .unwrap();
        let transaction_metadata_set = Arc::new(RwLock::new(TransactionMetadataSet::new()));
        let transaction_context = TransactionContext::new(
            &config,
            Arc::new(RwLock::new(usc)),
            transaction_metadata_set,
        );
        Ok(Self {
            blocks: Arc::new(RwLock::new(vec![])),
            mnemonic,
            wallet_options: Arc::new(RwLock::new(WalletOptions::default())),
            birthday: AtomicU64::new(height),
            verified_tree: Arc::new(RwLock::new(None)),
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            price: Arc::new(RwLock::new(WalletZecPriceInfo::new())),
            transaction_context,
        })
    }

    /// This is a Wallet constructor.  It is the internal function called by 2 LightWallet
    /// read procedures, by reducing its visibility we constrain possible uses.
    /// Each type that can be deserialized has an associated serialization version.  Our
    /// convention is to omit the type e.g. "wallet" from the local variable ident, and
    /// make explicit (via ident) which variable refers to a value deserialized from
    /// some source ("external") and which is represented as a source-code constant
    /// ("internal").

    pub(crate) async fn read_internal<R: Read>(
        mut reader: R,
        config: &ZingoConfig,
    ) -> io::Result<Self> {
        let external_version = reader.read_u64::<LittleEndian>()?;
        if external_version > Self::serialized_version() {
            let e = format!(
                "Don't know how to read wallet version {}. Do you have the latest version?\n{}",
                external_version,
                "Note: wallet files from zecwallet or beta zingo are not compatible"
            );
            error!("{}", e);
            return Err(io::Error::new(ErrorKind::InvalidData, e));
        }

        info!("Reading wallet version {}", external_version);
        let key = UnifiedSpendCapability::read(&mut reader, ())?;

        let mut blocks = Vector::read(&mut reader, |r| BlockData::read(r))?;
        if external_version <= 14 {
            // Reverse the order, since after version 20, we need highest-block-first
            // TODO: Consider order between 14 and 20.
            blocks = blocks.into_iter().rev().collect();
        }

        let transactions = if external_version <= 14 {
            TransactionMetadataSet::read_old(&mut reader)
        } else {
            TransactionMetadataSet::read(&mut reader)
        }?;

        let chain_name = utils::read_string(&mut reader)?;

        if chain_name != config.chain.to_string() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Wallet chain name {} doesn't match expected {}",
                    chain_name, config.chain
                ),
            ));
        }

        let wallet_options = if external_version <= 23 {
            WalletOptions::default()
        } else {
            WalletOptions::read(&mut reader)?
        };

        let birthday = reader.read_u64::<LittleEndian>()?;

        if external_version <= 22 {
            let _sapling_tree_verified = if external_version <= 12 {
                true
            } else {
                reader.read_u8()? == 1
            };
        }

        let verified_tree = if external_version <= 21 {
            None
        } else {
            Optional::read(&mut reader, |r| {
                use prost::Message;

                let buf = Vector::read(r, |r| r.read_u8())?;
                TreeState::decode(&buf[..]).map_err(|e| {
                    io::Error::new(
                        ErrorKind::InvalidData,
                        format!("Read Error: {}", e.to_string()),
                    )
                })
            })?
        };

        let price = if external_version <= 13 {
            WalletZecPriceInfo::new()
        } else {
            WalletZecPriceInfo::read(&mut reader)?
        };

        let transaction_context = TransactionContext::new(
            &config,
            Arc::new(RwLock::new(key)),
            Arc::new(RwLock::new(transactions)),
        );

        let _orchard_anchor_height_pairs = if external_version == 25 {
            Vector::read(&mut reader, |r| {
                let mut anchor_bytes = [0; 32];
                r.read_exact(&mut anchor_bytes)?;
                let block_height = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
                Ok((
                    Option::<Anchor>::from(Anchor::from_bytes(anchor_bytes))
                        .ok_or(Error::new(ErrorKind::InvalidData, "Bad orchard anchor"))?,
                    block_height,
                ))
            })?
        } else {
            Vec::new()
        };

        let seed_bytes = Vector::read(&mut reader, |r| r.read_u8())?;
        let mnemonic = Mnemonic::from_entropy(seed_bytes)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?;

        let mut lw = Self {
            blocks: Arc::new(RwLock::new(blocks)),
            mnemonic,
            wallet_options: Arc::new(RwLock::new(wallet_options)),
            birthday: AtomicU64::new(birthday),
            verified_tree: Arc::new(RwLock::new(verified_tree)),
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            price: Arc::new(RwLock::new(price)),
            transaction_context,
        };

        if external_version <= 14 {
            lw.set_witness_block_heights().await;
        }

        Ok(lw)
    }

    pub async fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        // Write the version
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // Write all the keys
        self.transaction_context
            .key
            .read()
            .await
            .write(&mut writer)?;

        Vector::write(&mut writer, &self.blocks.read().await, |w, b| b.write(w))?;

        self.transaction_context
            .transaction_metadata_set
            .read()
            .await
            .write(&mut writer)?;

        utils::write_string(
            &mut writer,
            &self.transaction_context.config.chain.to_string(),
        )?;

        self.wallet_options.read().await.write(&mut writer)?;

        // While writing the birthday, get it from the fn so we recalculate it properly
        // in case of rescans etc...
        writer.write_u64::<LittleEndian>(self.get_birthday().await)?;

        Optional::write(
            &mut writer,
            self.verified_tree.read().await.as_ref(),
            |w, t| {
                use prost::Message;
                let mut buf = vec![];

                t.encode(&mut buf)?;
                Vector::write(w, &buf, |w, b| w.write_u8(*b))
            },
        )?;

        // Price info
        self.price.read().await.write(&mut writer)?;

        Vector::write(
            &mut writer,
            &self.mnemonic.clone().into_entropy(),
            |w, byte| w.write_u8(*byte),
        )?;

        Ok(())
    }

    pub fn mnemonic(&self) -> &Mnemonic {
        &self.mnemonic
    }

    // Before version 20, witnesses didn't store their height, so we need to update them.
    pub async fn set_witness_block_heights(&mut self) {
        let top_height = self.last_synced_height().await;
        self.transaction_context
            .transaction_metadata_set
            .write()
            .await
            .current
            .iter_mut()
            .for_each(|(_, wtx)| {
                wtx.sapling_notes.iter_mut().for_each(|nd| {
                    nd.witnesses.top_height = top_height;
                });
            });
    }

    /*    pub fn keys(&self) -> Arc<RwLock<keys::Keys>> {
        todo!("Remove this")
        // compile_error!("Haven't gotten around to removing this yet")
    }*/

    pub fn unified_spend_capability(&self) -> Arc<RwLock<UnifiedSpendCapability>> {
        self.transaction_context.key.clone()
    }

    pub fn transactions(&self) -> Arc<RwLock<TransactionMetadataSet>> {
        self.transaction_context.transaction_metadata_set.clone()
    }

    pub async fn set_blocks(&self, new_blocks: Vec<BlockData>) {
        let mut blocks = self.blocks.write().await;
        blocks.clear();
        blocks.extend_from_slice(&new_blocks[..]);
    }

    /// Return a copy of the blocks currently in the wallet, needed to process possible reorgs
    pub async fn get_blocks(&self) -> Vec<BlockData> {
        self.blocks.read().await.iter().map(|b| b.clone()).collect()
    }

    pub(crate) fn note_address<D: DomainWalletExt>(
        network: &zingoconfig::ChainType,
        note: &D::WalletNote,
        unified_spend_auth: &UnifiedSpendCapability,
    ) -> String
    where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        note.fvk()
            .diversified_address(*note.diversifier())
            .and_then(|address| {
                D::ua_from_contained_receiver(unified_spend_auth, &address)
                    .map(|ua| ua.encode(network))
            })
            .unwrap_or("Diversifier not in wallet. Perhaps you restored from seed and didn't restore addresses".to_string())
    }

    pub async fn set_download_memo(&self, value: MemoDownloadOption) {
        self.wallet_options.write().await.download_memos = value;
    }

    pub async fn get_birthday(&self) -> u64 {
        let birthday = self.birthday.load(std::sync::atomic::Ordering::SeqCst);
        if birthday == 0 {
            self.get_first_transaction_block().await
        } else {
            cmp::min(self.get_first_transaction_block().await, birthday)
        }
    }

    pub async fn set_latest_zec_price(&self, price: f64) {
        if price <= 0 as f64 {
            warn!("Tried to set a bad current zec price {}", price);
            return;
        }

        self.price.write().await.zec_price = Some((now(), price));
        info!("Set current ZEC Price to USD {}", price);
    }

    // Get the current sending status.
    pub async fn get_send_progress(&self) -> SendProgress {
        self.send_progress.read().await.clone()
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

    // Reset the send progress status to blank
    async fn reset_send_progress(&self) {
        let mut g = self.send_progress.write().await;
        let next_id = g.id + 1;

        // Discard the old value, since we are replacing it
        let _ = std::mem::replace(&mut *g, SendProgress::new(next_id));
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
            .map(|wtx| u64::from(wtx.block_height))
            .min();

        let birthday = self.birthday.load(std::sync::atomic::Ordering::SeqCst);
        earliest_block // Returns optional, so if there's no transactions, it'll get the activation height
            .unwrap_or(cmp::max(
                birthday,
                self.transaction_context.config.sapling_activation_height(),
            ))
    }

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

    pub async fn set_initial_block(&self, height: u64, hash: &str, _sapling_tree: &str) -> bool {
        let mut blocks = self.blocks.write().await;
        if !blocks.is_empty() {
            return false;
        }

        blocks.push(BlockData::new_with(height, hash));

        true
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

    pub async fn last_synced_hash(&self) -> String {
        self.blocks
            .read()
            .await
            .first()
            .map(|block| block.hash())
            .unwrap_or_default()
    }

    async fn get_latest_wallet_height(&self) -> Option<u32> {
        self.blocks
            .read()
            .await
            .first()
            .map(|block| block.height as u32)
    }

    /// Determines the target height for a transaction, and the offset from which to
    /// select anchors, based on the current synchronised block chain.
    async fn get_target_height_and_anchor_offset(&self) -> Option<(u32, usize)> {
        match {
            let blocks = self.blocks.read().await;
            (
                blocks.last().map(|block| block.height as u32),
                blocks.first().map(|block| block.height as u32),
            )
        } {
            (Some(min_height), Some(max_height)) => {
                let target_height = max_height + 1;

                // Select an anchor ANCHOR_OFFSET back from the target block,
                // unless that would be before the earliest block we have.
                let anchor_height = cmp::max(
                    target_height
                        .saturating_sub(self.transaction_context.config.reorg_buffer_offset),
                    min_height,
                );

                Some((target_height, (target_height - anchor_height) as usize))
            }
            _ => None,
        }
    }

    /// Get the height of the anchor block
    pub async fn get_anchor_height(&self) -> u32 {
        match self.get_target_height_and_anchor_offset().await {
            Some((height, anchor_offset)) => height - anchor_offset as u32 - 1,
            None => return 0,
        }
    }

    pub fn memo_str(memo: Option<Memo>) -> Option<String> {
        match memo {
            Some(Memo::Text(m)) => Some(m.to_string()),
            Some(Memo::Arbitrary(_)) => Some("Wallet-internal memo".to_string()),
            _ => None,
        }
    }

    pub async fn maybe_verified_sapling_balance(&self, addr: Option<String>) -> u64 {
        self.shielded_balance::<ReceivedSaplingNoteAndMetadata>(addr, &[])
            .await
    }

    pub async fn maybe_verified_orchard_balance(&self, addr: Option<String>) -> u64 {
        self.shielded_balance::<ReceivedOrchardNoteAndMetadata>(addr, &[])
            .await
    }

    async fn shielded_balance<NnMd>(
        &self,
        target_addr: Option<String>,
        filters: &[Box<dyn Fn(&&NnMd, &TransactionMetadata) -> bool + '_>],
    ) -> u64
    where
        NnMd: traits::ReceivedNoteAndMetadata,
    {
        let filter_notes_by_target_addr = |notedata: &&NnMd| match target_addr.as_ref() {
            Some(addr) => {
                use self::traits::Recipient as _;
                let diversified_address = &notedata
                    .fvk()
                    .diversified_address(*notedata.diversifier())
                    .unwrap();
                *addr
                    == diversified_address
                        .b32encode_for_network(&self.transaction_context.config.chain)
            }
            None => true, // If the addr is none, then get all addrs.
        };
        self.transaction_context
            .transaction_metadata_set
            .read()
            .await
            .current
            .values()
            .map(|transaction| {
                let mut filtered_notes: Box<dyn Iterator<Item = &NnMd>> = Box::new(
                    NnMd::transaction_metadata_notes(transaction)
                        .iter()
                        .filter(filter_notes_by_target_addr),
                );
                // All filters in iterator are applied, by this loop
                for filtering_fn in filters {
                    filtered_notes =
                        Box::new(filtered_notes.filter(|nnmd| filtering_fn(nnmd, transaction)))
                }
                filtered_notes
                    .map(|notedata| {
                        if notedata.spent().is_none() && notedata.unconfirmed_spent().is_none() {
                            <NnMd as traits::ReceivedNoteAndMetadata>::value(notedata)
                        } else {
                            0
                        }
                    })
                    .sum::<u64>()
            })
            .sum::<u64>()
    }

    // Get all (unspent) utxos. Unconfirmed spent utxos are included
    pub async fn get_utxos(&self) -> Vec<Utxo> {
        self.transaction_context
            .transaction_metadata_set
            .read()
            .await
            .current
            .values()
            .flat_map(|transaction| transaction.utxos.iter().filter(|utxo| utxo.spent.is_none()))
            .map(|utxo| utxo.clone())
            .collect::<Vec<Utxo>>()
    }

    pub async fn tbalance(&self, addr: Option<String>) -> u64 {
        self.get_utxos()
            .await
            .iter()
            .filter(|utxo| match addr.as_ref() {
                Some(a) => utxo.address == *a,
                None => true,
            })
            .map(|utxo| utxo.value)
            .sum::<u64>()
    }

    /// The following functions use a filter/map functional approach to
    /// expressively unpack different kinds of transaction data.
    pub async fn unverified_sapling_balance(&self, target_addr: Option<String>) -> u64 {
        let anchor_height = self.get_anchor_height().await;

        let filters: &[Box<
            dyn Fn(&&ReceivedSaplingNoteAndMetadata, &TransactionMetadata) -> bool,
        >] = &[Box::new(|_, transaction: &TransactionMetadata| {
            transaction.block_height > BlockHeight::from_u32(anchor_height)
        })];
        self.shielded_balance(target_addr, filters).await
    }

    pub async fn unverified_orchard_balance(&self, target_addr: Option<String>) -> u64 {
        let anchor_height = self.get_anchor_height().await;

        let filters: &[Box<
            dyn Fn(&&ReceivedOrchardNoteAndMetadata, &TransactionMetadata) -> bool,
        >] = &[Box::new(|_, transaction: &TransactionMetadata| {
            transaction.block_height > BlockHeight::from_u32(anchor_height)
        })];
        self.shielded_balance(target_addr, filters).await
    }

    pub async fn verified_sapling_balance(&self, target_addr: Option<String>) -> u64 {
        self.verified_balance::<ReceivedSaplingNoteAndMetadata>(target_addr)
            .await
    }

    pub async fn verified_orchard_balance(&self, target_addr: Option<String>) -> u64 {
        self.verified_balance::<ReceivedOrchardNoteAndMetadata>(target_addr)
            .await
    }

    async fn verified_balance<NnMd: ReceivedNoteAndMetadata>(
        &self,
        target_addr: Option<String>,
    ) -> u64 {
        let anchor_height = self.get_anchor_height().await;
        let filters: &[Box<dyn Fn(&&NnMd, &TransactionMetadata) -> bool>] =
            &[Box::new(|_, transaction| {
                transaction.block_height <= BlockHeight::from_u32(anchor_height)
            })];
        self.shielded_balance::<NnMd>(target_addr, filters).await
    }

    pub async fn spendable_sapling_balance(&self, target_addr: Option<String>) -> u64 {
        let anchor_height = self.get_anchor_height().await;
        let filters: &[Box<
            dyn Fn(&&ReceivedSaplingNoteAndMetadata, &TransactionMetadata) -> bool,
        >] = &[
            Box::new(|_, transaction| {
                transaction.block_height <= BlockHeight::from_u32(anchor_height)
            }),
            Box::new(|nnmd, _| nnmd.witnesses.len() > 0),
        ];
        self.shielded_balance(target_addr, filters).await
    }

    pub async fn spendable_orchard_balance(&self, target_addr: Option<String>) -> u64 {
        let anchor_height = self.get_anchor_height().await;
        let filters: &[Box<
            dyn Fn(&&ReceivedOrchardNoteAndMetadata, &TransactionMetadata) -> bool,
        >] = &[
            Box::new(|_, transaction| {
                transaction.block_height <= BlockHeight::from_u32(anchor_height)
            }),
            Box::new(|nnmd, _| nnmd.witnesses.len() > 0),
        ];
        self.shielded_balance(target_addr, filters).await
    }

    ///TODO: Make this work for orchard too
    pub async fn decrypt_message(&self, enc: Vec<u8>) -> Option<Message> {
        let sapling_ivk = SaplingIvk::from(&*self.unified_spend_capability().read().await);

        if let Ok(msg) = Message::decrypt(&enc, &sapling_ivk) {
            // If decryption succeeded for this IVK, return the decrypted memo and the matched address
            return Some(msg);
        }

        // If nothing matched
        None
    }

    // Add the spent_at_height for each sapling note that has been spent. This field was added in wallet version 8,
    // so for older wallets, it will need to be added
    pub async fn fix_spent_at_height(&self) {
        // First, build an index of all the transaction_ids and the heights at which they were spent.
        let spent_transaction_id_map: HashMap<_, _> = self
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .current
            .iter()
            .map(|(transaction_id, wtx)| (transaction_id.clone(), wtx.block_height))
            .collect();

        // Go over all the sapling notes that might need updating
        self.transaction_context
            .transaction_metadata_set
            .write()
            .await
            .current
            .values_mut()
            .for_each(|wtx| {
                wtx.sapling_notes
                    .iter_mut()
                    .filter(|nd| nd.spent.is_some() && nd.spent.unwrap().1 == 0)
                    .for_each(|nd| {
                        let transaction_id = nd.spent.unwrap().0;
                        if let Some(height) =
                            spent_transaction_id_map.get(&transaction_id).map(|b| *b)
                        {
                            nd.spent = Some((transaction_id, height.into()));
                        }
                    })
            });

        // Go over all the Utxos that might need updating
        self.transaction_context
            .transaction_metadata_set
            .write()
            .await
            .current
            .values_mut()
            .for_each(|wtx| {
                wtx.utxos
                    .iter_mut()
                    .filter(|utxo| utxo.spent.is_some() && utxo.spent_at_height.is_none())
                    .for_each(|utxo| {
                        utxo.spent_at_height = spent_transaction_id_map
                            .get(&utxo.spent.unwrap())
                            .map(|b| u32::from(*b) as i32);
                    })
            });
    }

    async fn select_notes_and_utxos(
        &self,
        target_amount: Amount,
        transparent_only: bool,
        shield_transparent: bool,
        prefer_orchard_over_sapling: bool,
    ) -> (
        Vec<SpendableOrchardNote>,
        Vec<SpendableSaplingNote>,
        Vec<Utxo>,
        Amount,
    ) {
        // First, if we are allowed to pick transparent value, pick them all
        let utxos = if transparent_only || shield_transparent {
            self.get_utxos()
                .await
                .iter()
                .filter(|utxo| utxo.unconfirmed_spent.is_none() && utxo.spent.is_none())
                .map(|utxo| utxo.clone())
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        // Check how much we've selected
        let total_transparent_value = utxos.iter().fold(Amount::zero(), |prev, utxo| {
            (prev + Amount::from_u64(utxo.value).unwrap()).unwrap()
        });

        // If we are allowed only transparent funds or we've selected enough then return
        if transparent_only || total_transparent_value >= target_amount {
            return (vec![], vec![], utxos, total_transparent_value);
        }

        let mut sapling_value_selected = Amount::zero();
        let mut sapling_notes = vec![];
        // Select the minimum number of notes required to satisfy the target value
        if prefer_orchard_over_sapling {
            let sapling_candidates = self
                .get_all_domain_specific_notes::<SaplingDomain<zingoconfig::ChainType>>()
                .await;
            (sapling_notes, sapling_value_selected) =
                Self::add_notes_to_total::<SaplingDomain<zingoconfig::ChainType>>(
                    sapling_candidates,
                    (target_amount - total_transparent_value).unwrap(),
                );
            if total_transparent_value + sapling_value_selected >= Some(target_amount) {
                return (
                    vec![],
                    sapling_notes,
                    utxos,
                    (total_transparent_value + sapling_value_selected).unwrap(),
                );
            }
        }
        let orchard_candidates = self.get_all_domain_specific_notes::<OrchardDomain>().await;
        let (orchard_notes, orchard_value_selected) = Self::add_notes_to_total::<OrchardDomain>(
            orchard_candidates,
            (target_amount - total_transparent_value - sapling_value_selected).unwrap(),
        );
        if total_transparent_value + sapling_value_selected + orchard_value_selected
            >= Some(target_amount)
        {
            return (
                orchard_notes,
                sapling_notes,
                utxos,
                (total_transparent_value + sapling_value_selected + orchard_value_selected)
                    .unwrap(),
            );
        }
        if !prefer_orchard_over_sapling {
            let sapling_candidates = self
                .get_all_domain_specific_notes::<SaplingDomain<zingoconfig::ChainType>>()
                .await;
            (sapling_notes, sapling_value_selected) =
                Self::add_notes_to_total::<SaplingDomain<zingoconfig::ChainType>>(
                    sapling_candidates,
                    (target_amount - total_transparent_value).unwrap(),
                );
            if total_transparent_value + sapling_value_selected + orchard_value_selected
                >= Some(target_amount)
            {
                return (
                    orchard_notes,
                    sapling_notes,
                    utxos,
                    (total_transparent_value + sapling_value_selected + orchard_value_selected)
                        .unwrap(),
                );
            }
        }

        // If we can't select enough, then we need to return empty handed
        (vec![], vec![], vec![], Amount::zero())
    }

    async fn get_all_domain_specific_notes<D>(&self) -> Vec<D::SpendableNoteAT>
    where
        D: DomainWalletExt,
        <D as Domain>::Recipient: traits::Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        let usc_lth = self.unified_spend_capability();
        let usc = usc_lth.read().await;
        let tranmds_lth = self.transactions();
        let transaction_metadata_set = tranmds_lth.read().await;
        let mut candidate_notes = transaction_metadata_set
            .current
            .iter()
            .flat_map(|(transaction_id, transaction)| {
                D::WalletNote::transaction_metadata_notes(transaction)
                    .iter()
                    .map(move |note| (*transaction_id, note))
            })
            .filter(|(_, note)| note.value() > 0)
            .filter_map(|(transaction_id, note)| {
                // Filter out notes that are already spent
                if note.spent().is_some() || note.unconfirmed_spent().is_some() {
                    None
                } else {
                    // Get the spending key for the selected fvk, if we have it
                    let extsk = D::usc_to_sk(&usc);
                    SpendableNote::from(
                        transaction_id,
                        note,
                        self.transaction_context.config.reorg_buffer_offset as usize,
                        &Some(extsk),
                    )
                }
            })
            .collect::<Vec<D::SpendableNoteAT>>();
        candidate_notes.sort_unstable_by(|spendable_note_1, spendable_note_2| {
            D::WalletNote::value_from_note(&spendable_note_2.note())
                .cmp(&D::WalletNote::value_from_note(&spendable_note_1.note()))
        });
        candidate_notes
    }

    fn add_notes_to_total<D: DomainWalletExt>(
        candidates: Vec<D::SpendableNoteAT>,
        target_amount: Amount,
    ) -> (Vec<D::SpendableNoteAT>, Amount)
    where
        D::Note: PartialEq + Clone,
        D::Recipient: traits::Recipient,
    {
        let mut notes = Vec::new();
        let mut running_total = Amount::zero();
        for note in candidates {
            if running_total >= target_amount {
                break;
            }
            running_total += Amount::from_u64(D::WalletNote::value_from_note(&note.note()))
                .expect("Note value overflow error");
            notes.push(note);
        }

        (notes, running_total)
    }

    pub async fn send_to_address<F, Fut, P: TxProver>(
        &self,
        prover: P,
        transparent_only: bool,
        migrate_sapling_to_orchard: bool,
        tos: Vec<(&str, u64, Option<String>)>,
        submission_height: BlockHeight,
        broadcast_fn: F,
    ) -> Result<(String, Vec<u8>), String>
    where
        F: Fn(Box<[u8]>) -> Fut,
        Fut: Future<Output = Result<String, String>>,
    {
        // Reset the progress to start. Any errors will get recorded here
        self.reset_send_progress().await;

        // Call the internal function
        match self
            .send_to_address_inner(
                prover,
                transparent_only,
                migrate_sapling_to_orchard,
                tos,
                submission_height,
                broadcast_fn,
            )
            .await
        {
            Ok((transaction_id, raw_transaction)) => {
                self.set_send_success(transaction_id.clone()).await;
                Ok((transaction_id, raw_transaction))
            }
            Err(e) => {
                self.set_send_error(format!("{}", e)).await;
                Err(e)
            }
        }
    }

    async fn send_to_address_inner<F, Fut, P: TxProver>(
        &self,
        prover: P,
        transparent_only: bool,
        migrate_sapling_to_orchard: bool,
        tos: Vec<(&str, u64, Option<String>)>,
        submission_height: BlockHeight,
        broadcast_fn: F,
    ) -> Result<(String, Vec<u8>), String>
    where
        F: Fn(Box<[u8]>) -> Fut,
        Fut: Future<Output = Result<String, String>>,
    {
        let start_time = now();
        if tos.len() == 0 {
            return Err("Need at least one destination address".to_string());
        }

        let total_value = tos.iter().map(|to| to.1).sum::<u64>();
        println!(
            "0: Creating transaction sending {} ztoshis to {} addresses",
            total_value,
            tos.len()
        );

        // Convert address (str) to RecepientAddress and value to Amount
        let recipients = tos
            .iter()
            .map(|to| {
                let ra = match address::RecipientAddress::decode(
                    &self.transaction_context.config.chain,
                    to.0,
                ) {
                    Some(to) => to,
                    None => {
                        let e = format!("Invalid recipient address: '{}'", to.0);
                        error!("{}", e);
                        return Err(e);
                    }
                };

                let value = Amount::from_u64(to.1).unwrap();

                Ok((ra, value, to.2.clone()))
            })
            .collect::<Result<Vec<(address::RecipientAddress, Amount, Option<String>)>, String>>(
            )?;

        let destination_uas = recipients
            .iter()
            .filter_map(|recipient| match recipient.0 {
                address::RecipientAddress::Shielded(_) => None,
                address::RecipientAddress::Transparent(_) => None,
                address::RecipientAddress::Unified(ref ua) => Some(ua.clone()),
            })
            .collect::<Vec<_>>();

        // Select notes to cover the target value
        println!("{}: Selecting notes", now() - start_time);

        let target_amount = (Amount::from_u64(total_value).unwrap() + DEFAULT_FEE).unwrap();
        let latest_wallet_height = match self.get_latest_wallet_height().await {
            Some(h) => BlockHeight::from_u32(h),
            None => return Err("No blocks in wallet to target, please sync first".to_string()),
        };

        // Create a map from address -> sk for all taddrs, so we can spend from the
        // right address
        let address_to_sk = self
            .unified_spend_capability()
            .read()
            .await
            .get_taddr_to_secretkey_map(&self.transaction_context.config);

        let (orchard_notes, sapling_notes, utxos, selected_value) = self
            .select_notes_and_utxos(
                target_amount,
                transparent_only,
                true,
                migrate_sapling_to_orchard,
            )
            .await;
        if selected_value < target_amount {
            let e = format!(
                "Insufficient verified funds. Have {} zats, need {} zats. NOTE: funds need at least {} confirmations before they can be spent.",
                u64::from(selected_value), u64::from(target_amount), self.transaction_context.config
                .reorg_buffer_offset + 1
            );
            error!("{}", e);
            return Err(e);
        }
        println!("Selected notes worth {}", u64::from(selected_value));

        let orchard_anchor = self
            .get_orchard_anchor(&orchard_notes, latest_wallet_height)
            .await?;
        let mut builder = Builder::with_orchard_anchor(
            self.transaction_context.config.chain,
            submission_height,
            orchard_anchor,
        );
        println!(
            "{}: Adding {} sapling notes, {} orchard notes, and {} utxos",
            now() - start_time,
            sapling_notes.len(),
            orchard_notes.len(),
            utxos.len()
        );

        // Add all tinputs
        utxos
            .iter()
            .map(|utxo| {
                let outpoint: OutPoint = utxo.to_outpoint();

                let coin = TxOut {
                    value: Amount::from_u64(utxo.value).unwrap(),
                    script_pubkey: Script { 0: utxo.script.clone() },
                };

                match address_to_sk.get(&utxo.address) {
                    Some(sk) => builder.add_transparent_input(*sk, outpoint.clone(), coin.clone()),
                    None => {
                        // Something is very wrong
                        let e = format!("Couldn't find the secreykey for taddr {}", utxo.address);
                        error!("{}", e);

                        Err(zcash_primitives::transaction::builder::Error::TransparentBuild(
                            zcash_primitives::transaction::components::transparent::builder::Error::InvalidAddress,
                        ))
                    }
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("{:?}", e))?;

        for selected in sapling_notes.iter() {
            println!("Adding sapling spend");
            if let Err(e) = builder.add_sapling_spend(
                selected.extsk.clone(),
                selected.diversifier,
                selected.note.clone(),
                selected.witness.path().unwrap(),
            ) {
                let e = format!("Error adding note: {:?}", e);
                error!("{}", e);
                return Err(e);
            }
        }

        for selected in orchard_notes.iter() {
            println!("Adding orchard spend");
            let path = selected.witness.path().unwrap();
            if let Err(e) = builder.add_orchard_spend(
                selected.spend_key.clone(),
                selected.note.clone(),
                orchard::tree::MerklePath::from((
                    incrementalmerkletree::Position::from(path.position as usize),
                    path.auth_path
                        .iter()
                        .map(|(node, _)| node.clone())
                        .collect(),
                )),
            ) {
                let e = format!("Error adding note: {:?}", e);
                error!("{}", e);
                return Err(e);
            }
        }

        // We'll use the first ovk to encrypt outgoing transactions
        let sapling_ovk = zcash_primitives::keys::OutgoingViewingKey::from(
            &*self.unified_spend_capability().read().await,
        );
        let orchard_ovk =
            orchard::keys::OutgoingViewingKey::from(&*self.unified_spend_capability().read().await);

        let mut total_z_recipients = 0u32;
        for (recipient_address, value, memo) in recipients {
            // Compute memo if it exists
            let validated_memo = match memo {
                None => MemoBytes::from(Memo::Empty),
                Some(s) => {
                    // If the string starts with an "0x", and contains only hex chars ([a-f0-9]+) then
                    // interpret it as a hex
                    match utils::interpret_memo_string(s) {
                        Ok(m) => m,
                        Err(e) => {
                            error!("{}", e);
                            return Err(e);
                        }
                    }
                }
            };

            println!("{}: Adding output", now() - start_time);

            if let Err(e) = match recipient_address {
                address::RecipientAddress::Shielded(to) => {
                    total_z_recipients += 1;
                    builder.add_sapling_output(Some(sapling_ovk), to.clone(), value, validated_memo)
                }
                address::RecipientAddress::Transparent(to) => {
                    builder.add_transparent_output(&to, value)
                }
                address::RecipientAddress::Unified(ua) => {
                    if let Some(orchard_addr) = ua.orchard() {
                        builder.add_orchard_output(
                            Some(orchard_ovk.clone()),
                            orchard_addr.clone(),
                            u64::from(value),
                            validated_memo,
                        )
                    } else if let Some(sapling_addr) = ua.sapling() {
                        total_z_recipients += 1;
                        builder.add_sapling_output(
                            Some(sapling_ovk),
                            sapling_addr.clone(),
                            value,
                            validated_memo,
                        )
                    } else {
                        return Err("Received UA with no Orchard or Sapling receiver".to_string());
                    }
                }
            } {
                let e = format!("Error adding output: {:?}", e);
                error!("{}", e);
                return Err(e);
            }
        }
        let uas_bytes = match create_wallet_internal_memo_version_0(&destination_uas) {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!(
                    "Could not write uas to memo field: {e}\n\
        Your wallet will display an incorrect sent-to address. This is a visual error only.\n\
        The correct address was sent to."
                );
                [0; 511]
            }
        };

        dbg!(selected_value, target_amount);
        if let Err(e) = builder.add_orchard_output(
            Some(orchard_ovk.clone()),
            *self.unified_spend_capability().read().await.addresses()[0]
                .orchard()
                .unwrap(),
            dbg!(u64::from(selected_value) - u64::from(target_amount)),
            // Here we store the uas we sent to in the memo field.
            // These are used to recover the full UA we sent to.
            MemoBytes::from(Memo::Arbitrary(Box::new(uas_bytes))),
        ) {
            let e = format!("Error adding change output: {:?}", e);
            error!("{}", e);
            return Err(e);
        }

        // Set up a channel to recieve updates on the progress of building the transaction.
        let (transmitter, receiver) = channel::<Progress>();
        let progress = self.send_progress.clone();

        // Use a separate thread to handle sending from std::mpsc to tokio::sync::mpsc
        let (transmitter2, mut receiver2) = tokio::sync::mpsc::unbounded_channel();
        std::thread::spawn(move || {
            while let Ok(r) = receiver.recv() {
                transmitter2.send(r.cur()).unwrap();
            }
        });

        let progress_handle = tokio::spawn(async move {
            while let Some(r) = receiver2.recv().await {
                println!("{}: Progress: {r}", now() - start_time);
                progress.write().await.progress = r;
            }

            progress.write().await.is_send_in_progress = false;
        });

        {
            let mut p = self.send_progress.write().await;
            p.is_send_in_progress = true;
            p.progress = 0;
            p.total = sapling_notes.len() as u32 + total_z_recipients;
        }

        println!("{}: Building transaction", now() - start_time);

        builder.with_progress_notifier(transmitter);
        let (transaction, _) = match builder.build(&prover) {
            Ok(res) => res,
            Err(e) => {
                let e = format!("Error creating transaction: {:?}", e);
                error!("{}", e);
                self.send_progress.write().await.is_send_in_progress = false;
                return Err(e);
            }
        };

        // Wait for all the progress to be updated
        progress_handle.await.unwrap();

        println!("{}: Transaction created", now() - start_time);
        println!("Transaction ID: {}", transaction.txid());

        {
            self.send_progress.write().await.is_send_in_progress = false;
        }

        // Create the transaction bytes
        let mut raw_transaction = vec![];
        transaction.write(&mut raw_transaction).unwrap();

        let transaction_id = broadcast_fn(raw_transaction.clone().into_boxed_slice()).await?;

        // Mark notes as spent.
        {
            // Mark sapling notes as unconfirmed spent
            let mut transactions = self
                .transaction_context
                .transaction_metadata_set
                .write()
                .await;
            for selected in sapling_notes {
                let mut spent_note = transactions
                    .current
                    .get_mut(&selected.transaction_id)
                    .unwrap()
                    .sapling_notes
                    .iter_mut()
                    .find(|nd| nd.nullifier == selected.nullifier)
                    .unwrap();
                spent_note.unconfirmed_spent =
                    Some((transaction.txid(), u32::from(submission_height)));
            }
            // Mark orchard notes as unconfirmed spent
            for selected in orchard_notes {
                let mut spent_note = transactions
                    .current
                    .get_mut(&selected.transaction_id)
                    .unwrap()
                    .orchard_notes
                    .iter_mut()
                    .find(|nd| nd.nullifier == selected.nullifier)
                    .unwrap();
                spent_note.unconfirmed_spent =
                    Some((transaction.txid(), u32::from(submission_height)));
            }

            // Mark this utxo as unconfirmed spent
            for utxo in utxos {
                let mut spent_utxo = transactions
                    .current
                    .get_mut(&utxo.txid)
                    .unwrap()
                    .utxos
                    .iter_mut()
                    .find(|u| utxo.txid == u.txid && utxo.output_index == u.output_index)
                    .unwrap();
                spent_utxo.unconfirmed_spent =
                    Some((transaction.txid(), u32::from(submission_height)));
            }
        }

        // Add this transaction to the mempool structure
        {
            let price = self.price.read().await.clone();

            self.transaction_context
                .scan_full_tx(
                    transaction,
                    submission_height.into(),
                    true,
                    now() as u32,
                    TransactionMetadata::get_price(now(), &price),
                )
                .await;
        }

        Ok((transaction_id, raw_transaction))
    }

    async fn get_orchard_anchor(
        &self,
        orchard_notes: &[SpendableOrchardNote],
        target_height: BlockHeight,
    ) -> Result<Anchor, String> {
        if let Some(note) = orchard_notes.get(0) {
            Ok(orchard::Anchor::from(note.witness.root()))
        } else {
            let trees = crate::grpc_connector::GrpcConnector::get_trees(
                self.transaction_context
                    .config
                    .server_uri
                    .read()
                    .unwrap()
                    .clone(),
                u64::from(target_height)
                    - self.transaction_context.config.reorg_buffer_offset as u64,
            )
            .await?;
            let orchard_tree = CommitmentTree::<MerkleHashOrchard>::read(
                hex::decode(&trees.orchard_tree).unwrap().as_slice(),
            )
            .unwrap_or(CommitmentTree::empty());
            Ok(Anchor::from(orchard_tree.root()))
        }
    }
}

//This function will likely be used again if/when we re-implement key import
#[allow(dead_code)]
fn decode_orchard_spending_key(
    expected_hrp: &str,
    s: &str,
) -> Result<Option<OrchardSpendingKey>, String> {
    match bech32::decode(&s) {
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
    use orchard::tree::MerkleHashOrchard;
    use zcash_primitives::{merkle_tree::CommitmentTree, transaction::components::Amount};

    use crate::{
        apply_scenario,
        blaze::test_utils::{incw_to_string, FakeTransaction},
        lightclient::test_server::{
            clean_shutdown, mine_numblocks_each_with_two_sap_txs, mine_pending_blocks,
            NBlockFCBLScenario,
        },
        wallet::keys::unified::get_transparent_secretkey_pubkey_taddr,
    };

    mod bench_select_notes_and_utxos {
        use super::*;
        crate::apply_scenario! {insufficient_funds_0_present_needed_1 10}
        async fn insufficient_funds_0_present_needed_1(scenario: NBlockFCBLScenario) {
            let NBlockFCBLScenario { lightclient, .. } = scenario;
            let sufficient_funds = lightclient
                .wallet
                .select_notes_and_utxos(Amount::from_u64(1).unwrap(), false, false, false)
                .await;
            assert_eq!(Amount::from_u64(0).unwrap(), sufficient_funds.3);
        }

        crate::apply_scenario! {sufficient_funds_1_present_needed_1 10}
        async fn sufficient_funds_1_present_needed_1(scenario: NBlockFCBLScenario) {
            let NBlockFCBLScenario {
                lightclient,
                data,
                mut fake_compactblock_list,
                ..
            } = scenario;
            let extended_fvk = zcash_primitives::zip32::ExtendedFullViewingKey::from(
                &*lightclient.wallet.unified_spend_capability().read().await,
            );
            let (_, _, _) =
                fake_compactblock_list.create_sapling_coinbase_transaction(&extended_fvk, 1);
            mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
            assert_eq!(
                lightclient
                    .wallet
                    .maybe_verified_sapling_balance(None)
                    .await,
                1
            );
            let sufficient_funds = lightclient
                .wallet
                .select_notes_and_utxos(Amount::from_u64(1).unwrap(), false, false, false)
                .await;
            assert_eq!(Amount::from_u64(1).unwrap(), sufficient_funds.3);
        }
        crate::apply_scenario! {sufficient_funds_1_plus_txfee_present_needed_1 10}
        async fn sufficient_funds_1_plus_txfee_present_needed_1(scenario: NBlockFCBLScenario) {
            let NBlockFCBLScenario {
                lightclient,
                data,
                mut fake_compactblock_list,
                ..
            } = scenario;
            let extended_fvk = zcash_primitives::zip32::ExtendedFullViewingKey::from(
                &*lightclient.wallet.unified_spend_capability().read().await,
            );
            use zcash_primitives::transaction::components::amount::DEFAULT_FEE;
            let (_, _, _) = fake_compactblock_list
                .create_sapling_coinbase_transaction(&extended_fvk, 1 + u64::from(DEFAULT_FEE));
            for _ in 0..=3 {
                fake_compactblock_list.add_empty_block();
            }
            mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;
            assert_eq!(
                lightclient
                    .wallet
                    .maybe_verified_sapling_balance(None)
                    .await,
                1_001
            );
            let sufficient_funds = lightclient
                .wallet
                .select_notes_and_utxos(Amount::from_u64(1).unwrap(), false, false, false)
                .await;
            assert_eq!(Amount::from_u64(1_001).unwrap(), sufficient_funds.3);
        }
    }
    apply_scenario! {z_t_note_selection 10}
    async fn z_t_note_selection(scenario: NBlockFCBLScenario) {
        let NBlockFCBLScenario {
            data,
            mut lightclient,
            mut fake_compactblock_list,
            ..
        } = scenario;
        // 2. Send an incoming transaction to fill the wallet
        let extfvk1 = zcash_primitives::zip32::ExtendedFullViewingKey::from(
            &*lightclient.wallet.unified_spend_capability().read().await,
        );
        let value = 100_000;
        let (transaction, _height, _) =
            fake_compactblock_list.create_sapling_coinbase_transaction(&extfvk1, value);
        let txid = transaction.txid();
        mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

        assert_eq!(lightclient.wallet.last_synced_height().await, 11);

        // 3. With one confirmation, we should be able to select the note
        let amt = Amount::from_u64(10_000).unwrap();
        // Reset the anchor offsets
        lightclient
            .wallet
            .transaction_context
            .config
            .reorg_buffer_offset = 0;
        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, false, false)
            .await;
        assert!(selected >= amt);
        assert_eq!(sapling_notes.len(), 1);
        assert_eq!(sapling_notes[0].note.value, value);
        assert_eq!(utxos.len(), 0);
        assert_eq!(
            incw_to_string(&sapling_notes[0].witness),
            incw_to_string(
                lightclient
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .read()
                    .await
                    .current
                    .get(&txid)
                    .unwrap()
                    .sapling_notes[0]
                    .witnesses
                    .last()
                    .unwrap()
            )
        );

        // With min anchor_offset at 1, we can't select any notes
        lightclient
            .wallet
            .transaction_context
            .config
            .reorg_buffer_offset = 1;
        let (_orchard_notes, sapling_notes, utxos, _selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, false, false)
            .await;
        assert_eq!(sapling_notes.len(), 0);
        assert_eq!(utxos.len(), 0);

        // Mine 1 block, then it should be selectable
        mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 1)
            .await;

        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, false, false)
            .await;
        assert!(selected >= amt);
        assert_eq!(sapling_notes.len(), 1);
        assert_eq!(sapling_notes[0].note.value, value);
        assert_eq!(utxos.len(), 0);
        assert_eq!(
            incw_to_string(&sapling_notes[0].witness),
            incw_to_string(
                lightclient
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .read()
                    .await
                    .current
                    .get(&txid)
                    .unwrap()
                    .sapling_notes[0]
                    .witnesses
                    .get_from_last(1)
                    .unwrap()
            )
        );

        // Mine 15 blocks, then selecting the note should result in witness only 10 blocks deep
        mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 15)
            .await;
        lightclient
            .wallet
            .transaction_context
            .config
            .reorg_buffer_offset = 1;
        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, true, false)
            .await;
        assert!(selected >= amt);
        assert_eq!(sapling_notes.len(), 1);
        assert_eq!(sapling_notes[0].note.value, value);
        assert_eq!(utxos.len(), 0);
        assert_eq!(
            incw_to_string(&sapling_notes[0].witness),
            incw_to_string(
                lightclient
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .read()
                    .await
                    .current
                    .get(&txid)
                    .unwrap()
                    .sapling_notes[0]
                    .witnesses
                    .get_from_last(1)
                    .unwrap()
            )
        );

        // Trying to select a large amount will fail
        let amt = Amount::from_u64(1_000_000).unwrap();
        let (_orchard_notes, sapling_notes, utxos, _selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, false, false)
            .await;
        assert_eq!(sapling_notes.len(), 0);
        assert_eq!(utxos.len(), 0);

        // 4. Get an incoming transaction to a t address
        let (_sk, pk, taddr) = get_transparent_secretkey_pubkey_taddr(&lightclient).await;
        let tvalue = 100_000;

        let mut fake_transaction = FakeTransaction::new(true);
        fake_transaction.add_t_output(&pk, taddr, tvalue);
        let (_ttransaction, _) = fake_compactblock_list.add_fake_transaction(fake_transaction);
        mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

        // Trying to select a large amount will now succeed
        let amt = Amount::from_u64(value + tvalue - 10_000).unwrap();
        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, true, false)
            .await;
        assert_eq!(selected, Amount::from_u64(value + tvalue).unwrap());
        assert_eq!(sapling_notes.len(), 1);
        assert_eq!(utxos.len(), 1);

        // If we set transparent-only = true, only the utxo should be selected
        let amt = Amount::from_u64(tvalue - 10_000).unwrap();
        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, true, true, false)
            .await;
        assert_eq!(selected, Amount::from_u64(tvalue).unwrap());
        assert_eq!(sapling_notes.len(), 0);
        assert_eq!(utxos.len(), 1);

        // Set min confs to 5, so the sapling note will not be selected
        lightclient
            .wallet
            .transaction_context
            .config
            .reorg_buffer_offset = 4;
        let amt = Amount::from_u64(tvalue - 10_000).unwrap();
        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, true, false)
            .await;
        assert_eq!(selected, Amount::from_u64(tvalue).unwrap());
        assert_eq!(sapling_notes.len(), 0);
        assert_eq!(utxos.len(), 1);
    }

    apply_scenario! {multi_z_note_selection 10}
    async fn multi_z_note_selection(scenario: NBlockFCBLScenario) {
        let NBlockFCBLScenario {
            data,
            mut lightclient,
            mut fake_compactblock_list,
            ..
        } = scenario;
        // 2. Send an incoming transaction to fill the wallet
        let extfvk1 = zcash_primitives::zip32::ExtendedFullViewingKey::from(
            &*lightclient.wallet.unified_spend_capability().read().await,
        );
        let value1 = 100_000;
        let (transaction, _height, _) =
            fake_compactblock_list.create_sapling_coinbase_transaction(&extfvk1, value1);
        let txid = transaction.txid();
        mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

        assert_eq!(lightclient.wallet.last_synced_height().await, 11);

        // 3. With one confirmation, we should be able to select the note
        let amt = Amount::from_u64(10_000).unwrap();
        // This test relies on historic functionality of multiple anchor offsets
        lightclient
            .wallet
            .transaction_context
            .config
            .reorg_buffer_offset = 0;
        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, false, false)
            .await;
        assert!(selected >= amt);
        assert_eq!(sapling_notes.len(), 1);
        assert_eq!(sapling_notes[0].note.value, value1);
        assert_eq!(utxos.len(), 0);
        assert_eq!(
            incw_to_string(&sapling_notes[0].witness),
            incw_to_string(
                lightclient
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .read()
                    .await
                    .current
                    .get(&txid)
                    .unwrap()
                    .sapling_notes[0]
                    .witnesses
                    .last()
                    .unwrap()
            )
        );

        // Mine 5 blocks
        mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 5)
            .await;

        // Reset the anchor offsets
        lightclient
            .wallet
            .transaction_context
            .config
            .reorg_buffer_offset = 4;

        // 4. Send another incoming transaction.
        let value2 = 200_000;
        let (_transaction, _height, _) =
            fake_compactblock_list.create_sapling_coinbase_transaction(&extfvk1, value2);
        mine_pending_blocks(&mut fake_compactblock_list, &data, &lightclient).await;

        // Now, try to select a small amount, it should prefer the older note
        let amt = Amount::from_u64(10_000).unwrap();
        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, false, false)
            .await;
        assert!(selected >= amt);
        assert_eq!(sapling_notes.len(), 1);
        assert_eq!(sapling_notes[0].note.value, value1);
        assert_eq!(utxos.len(), 0);

        // Selecting a bigger amount should select both notes
        // This test relies on historic functionality of multiple anchor offsets
        lightclient
            .wallet
            .transaction_context
            .config
            .reorg_buffer_offset = 0;
        let amt = Amount::from_u64(value1 + value2).unwrap();
        let (_orchard_notes, sapling_notes, utxos, selected) = lightclient
            .wallet
            .select_notes_and_utxos(amt, false, false, false)
            .await;
        assert!(selected == amt);
        assert_eq!(sapling_notes.len(), 2);
        assert_eq!(utxos.len(), 0);
    }

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
        let mut orchard_tree = CommitmentTree::empty();
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

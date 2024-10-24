//! This mod contains write and read functionality of impl LightWallet
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use log::{error, info};
use zcash_keys::keys::UnifiedSpendingKey;
use zip32::AccountId;

use std::{
    collections::HashMap,
    io::{self, Error, ErrorKind, Read, Write},
    sync::{atomic::AtomicU64, Arc},
};

#[cfg(feature = "sync")]
use std::collections::BTreeMap;
use tokio::sync::RwLock;

use bip0039::Mnemonic;

use zcash_client_backend::proto::service::TreeState;
use zcash_encoding::{Optional, Vector};

use zcash_primitives::consensus::BlockHeight;

use crate::{config::ZingoConfig, wallet::keys::unified::UnifiedKeyStore};

use crate::wallet::traits::ReadableWriteable;
use crate::wallet::WalletOptions;
use crate::wallet::{utils, SendProgress};

use super::keys::unified::WalletCapability;

use super::LightWallet;
use super::{
    data::{BlockData, WalletZecPriceInfo},
    transaction_context::TransactionContext,
    tx_map::TxMap,
};

impl LightWallet {
    /// Changes in version 30:
    /// - New WalletCapability version (v4) which implements read/write for rejection addresses
    pub const fn serialized_version() -> u64 {
        30
    }

    /// TODO: Add Doc Comment Here!
    pub async fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        // Write the version
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // Write all the keys
        self.transaction_context
            .key
            .write(&mut writer, self.transaction_context.config.chain)?;

        Vector::write(&mut writer, &self.last_100_blocks.read().await, |w, b| {
            b.write(w)
        })?;

        self.transaction_context
            .transaction_metadata_set
            .write()
            .await
            .write(&mut writer)
            .await?;

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

        let seed_bytes = match &self.mnemonic {
            Some(m) => m.0.clone().into_entropy(),
            None => vec![],
        };
        Vector::write(&mut writer, &seed_bytes, |w, byte| w.write_u8(*byte))?;

        if let Some(m) = &self.mnemonic {
            writer.write_u32::<LittleEndian>(m.1)?;
        }

        Ok(())
    }

    /// This is a Wallet constructor.  It is the internal function called by 2 LightWallet
    /// read procedures, by reducing its visibility we constrain possible uses.
    /// Each type that can be deserialized has an associated serialization version.  Our
    /// convention is to omit the type e.g. "wallet" from the local variable ident, and
    /// make explicit (via ident) which variable refers to a value deserialized from
    /// some source ("external") and which is represented as a source-code constant
    /// ("internal").
    pub async fn read_internal<R: Read>(mut reader: R, config: &ZingoConfig) -> io::Result<Self> {
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
        let mut wallet_capability = WalletCapability::read(&mut reader, config.chain)?;

        let mut blocks = Vector::read(&mut reader, |r| BlockData::read(r))?;
        if external_version <= 14 {
            // Reverse the order, since after version 20, we need highest-block-first
            // TODO: Consider order between 14 and 20.
            blocks = blocks.into_iter().rev().collect();
        }

        let transactions = if external_version <= 14 {
            TxMap::read_old(&mut reader, &wallet_capability)
        } else {
            TxMap::read(&mut reader, &wallet_capability)
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
                TreeState::decode(&buf[..])
                    .map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))
            })?
        };

        let price = if external_version <= 13 {
            WalletZecPriceInfo::default()
        } else {
            WalletZecPriceInfo::read(&mut reader)?
        };

        let _orchard_anchor_height_pairs = if external_version == 25 {
            Vector::read(&mut reader, |r| {
                let mut anchor_bytes = [0; 32];
                r.read_exact(&mut anchor_bytes)?;
                let block_height = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
                Ok((
                    Option::<orchard::Anchor>::from(orchard::Anchor::from_bytes(anchor_bytes))
                        .ok_or(Error::new(ErrorKind::InvalidData, "Bad orchard anchor"))?,
                    block_height,
                ))
            })?
        } else {
            Vec::new()
        };

        let seed_bytes = Vector::read(&mut reader, |r| r.read_u8())?;
        let mnemonic = if !seed_bytes.is_empty() {
            let account_index = if external_version >= 28 {
                reader.read_u32::<LittleEndian>()?
            } else {
                0
            };
            Some((
                Mnemonic::from_entropy(seed_bytes)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?,
                account_index,
            ))
        } else {
            None
        };

        // Derive unified spending key from seed and overide temporary USK if wallet is pre v29.
        //
        // UnifiedSpendingKey is initially incomplete for old wallet versions.
        // This is due to the legacy transparent extended private key (ExtendedPrivKey) not containing all information required for BIP0032.
        // There is also the issue that the legacy transparent private key is derived an extra level to the external scope.
        if external_version < 29 {
            if let Some(mnemonic) = mnemonic.as_ref() {
                wallet_capability.set_unified_key_store(UnifiedKeyStore::Spend(Box::new(
                    UnifiedSpendingKey::from_seed(
                        &config.chain,
                        &mnemonic.0.to_seed(""),
                        AccountId::ZERO,
                    )
                    .map_err(|e| {
                        Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "Failed to derive unified spending key from stored seed bytes. {}",
                                e
                            ),
                        )
                    })?,
                )));
            } else if let UnifiedKeyStore::Spend(_) = wallet_capability.unified_key_store() {
                return Err(io::Error::new(
                    ErrorKind::Other,
                    "loading from legacy spending keys with no seed phrase to recover",
                ));
            }
        }

        info!("Keys in this wallet:");
        match wallet_capability.unified_key_store() {
            UnifiedKeyStore::Spend(_) => {
                info!("  - orchard spending key");
                info!("  - sapling extended spending key");
                info!("  - transparent extended private key");
            }
            UnifiedKeyStore::View(ufvk) => {
                if ufvk.orchard().is_some() {
                    info!("  - orchard full viewing key");
                }
                if ufvk.sapling().is_some() {
                    info!("  - sapling diversifiable full viewing key");
                }
                if ufvk.transparent().is_some() {
                    info!("  - transparent extended public key");
                }
            }
            UnifiedKeyStore::Empty => info!("  - no keys found"),
        }

        // this initialization combines two types of data
        let transaction_context = TransactionContext::new(
            // Config data could be used differently based on the circumstances
            // hardcoded?
            // entered at init by user?
            // stored on disk in a separate location and connected by a descendant library (such as zingo-mobile)?
            config,
            // Saveable Arc data
            //   - Arcs allow access between threads.
            //   - This data is loaded from the wallet file and but needs multithreaded access during sync.
            Arc::new(wallet_capability),
            Arc::new(RwLock::new(transactions)),
        );

        let lw = Self {
            last_100_blocks: Arc::new(RwLock::new(blocks)),
            mnemonic,
            wallet_options: Arc::new(RwLock::new(wallet_options)),
            birthday: AtomicU64::new(birthday),
            verified_tree: Arc::new(RwLock::new(verified_tree)),
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            price: Arc::new(RwLock::new(price)),
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
        };

        Ok(lw)
    }
}

#[cfg(any(test, feature = "test-elevation"))]
pub mod testing;

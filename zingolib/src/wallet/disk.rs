//! This mod contains write and read functionality of impl LightWallet
use append_only_vec::AppendOnlyVec;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use log::{error, info};
use pepper_sync::keys::transparent::TransparentScope;
use zcash_keys::keys::UnifiedSpendingKey;
use zip32::AccountId;

use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Error, ErrorKind, Read, Write},
    sync::Arc,
};

use tokio::sync::RwLock;

use bip0039::Mnemonic;

use zcash_client_backend::proto::service::TreeState;
use zcash_encoding::{Optional, Vector};

use zcash_primitives::consensus::{self, BlockHeight};

use crate::{config::ChainType, wallet::keys::unified::UnifiedKeyStore};

use crate::wallet::traits::ReadableWriteable;
use crate::wallet::WalletOptions;
use crate::wallet::{utils, SendProgress};

use super::keys::unified::{ReceiverSelection, WalletCapability};

use super::LightWallet;
use super::{
    data::{BlockData, WalletZecPriceInfo},
    tx_map::TxMap,
};

impl LightWallet {
    /// Changes in version 32:
    /// - Wallet restructure due to integration of new sync engine
    pub const fn serialized_version() -> u64 {
        32 // FIXME: double check this is correctly incremented before sync integration is complete
    }

    /// Serialize into `writer`
    // FIXME: remove arc mutex on price and options and make sync fn
    pub async fn write<W: Write>(
        &mut self,
        mut writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;
        utils::write_string(&mut writer, &self.network.to_string())?;
        let seed_bytes = match &self.mnemonic {
            Some(m) => m.0.clone().into_entropy(),
            None => vec![],
        };
        Vector::write(&mut writer, &seed_bytes, |w, byte| w.write_u8(*byte))?;
        if let Some(m) = &self.mnemonic {
            writer.write_u32::<LittleEndian>(m.1)?;
        }
        writer.write_u32::<LittleEndian>(self.birthday.into())?;
        self.unified_key_store.write(&mut writer, self.network)?;

        // TODO: consider whether its worth tracking receiver selections. if so, we need to store them in encoded memos.
        Vector::write(
            &mut writer,
            &self.unified_addresses.iter().collect::<Vec<_>>(),
            |w, address| {
                ReceiverSelection {
                    orchard: address.orchard().is_some(),
                    sapling: address.sapling().is_some(),
                    transparent: address.transparent().is_some(),
                }
                .write(w, ())
            },
        )?;
        for scope in [
            TransparentScope::External,
            TransparentScope::Internal,
            TransparentScope::Refund,
        ] {
            let transparent_address_count = self
                .transparent_addresses
                .keys()
                .filter(|address_id| address_id.scope() == scope)
                .map(|address_id| address_id.address_index())
                .max()
                .map(|max_index| max_index + 1)
                .unwrap_or(0);
            writer.write_u32::<LittleEndian>(transparent_address_count)?;
        }

        Vector::write(
            &mut writer,
            &self.wallet_blocks.values().collect::<Vec<_>>(),
            |w, &block| block.write(w),
        )?;
        Vector::write(
            &mut writer,
            &self.wallet_transactions.values().collect::<Vec<_>>(),
            |w, &transaction| transaction.write(w, consensus_parameters),
        )?;
        self.nullifier_map.write(&mut writer)?;
        Vector::write(
            &mut writer,
            &self.outpoint_map.iter().collect::<Vec<_>>(),
            |w, (&output_id, &locator)| {
                output_id.txid().write(&mut *w)?;
                w.write_u16::<LittleEndian>(output_id.output_index())?;
                w.write_u32::<LittleEndian>(locator.0.into())?;
                locator.1.write(w)
            },
        )?;
        self.shard_trees.write(&mut writer)?;
        self.sync_state.write(&mut writer)?;

        self.wallet_options.read().await.write(&mut writer)?;
        self.price.read().await.write(&mut writer)
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R, network: ChainType) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        info!("Reading wallet version {}", version);
        match version {
            ..32 => Self::read_v0(reader, network, version),
            32 => Self::read_v32(reader, network),
            _ => {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "Failed to read wallet version {}. Do you have the latest version?\n{}",
                        version,
                        "Note: wallet files from zecwallet or beta zingo are not compatible"
                    ),
                ));
            }
        }
    }

    fn read_v0<R: Read>(mut reader: R, network: ChainType, version: u64) -> io::Result<Self> {
        let mut wallet_capability = WalletCapability::read(&mut reader, network)?;
        let mut _blocks = Vector::read(&mut reader, |r| BlockData::read(r))?;
        let _transactions = if version <= 14 {
            TxMap::read_old(&mut reader, &wallet_capability)?
        } else {
            TxMap::read(&mut reader, &wallet_capability)?
        };

        let chain_name = utils::read_string(&mut reader)?;
        if chain_name != network.to_string() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Wallet chain name {} doesn't match expected {}",
                    chain_name, network
                ),
            ));
        }

        let wallet_options = if version <= 23 {
            WalletOptions::default()
        } else {
            WalletOptions::read(&mut reader)?
        };
        let birthday = BlockHeight::from_u32(
            reader
                .read_u64::<LittleEndian>()?
                .try_into()
                .expect("should never overflow"),
        );

        if version <= 22 {
            let _sapling_tree_verified = if version <= 12 {
                true
            } else {
                reader.read_u8()? == 1
            };
        }
        let _verified_tree = if version <= 21 {
            None
        } else {
            Optional::read(&mut reader, |r| {
                use prost::Message;

                let buf = Vector::read(r, |r| r.read_u8())?;
                TreeState::decode(&buf[..])
                    .map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))
            })?
        };

        let price = if version <= 13 {
            WalletZecPriceInfo::default()
        } else {
            WalletZecPriceInfo::read(&mut reader)?
        };

        let _orchard_anchor_height_pairs = if version == 25 {
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
            let account_index = if version >= 28 {
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

        // Derive unified spending key from seed and override temporary USK if wallet is pre v29.
        //
        // UnifiedSpendingKey is initially incomplete for old wallet versions.
        // This is due to the legacy transparent extended private key (ExtendedPrivKey) not containing all information required for BIP0032.
        // There is also the issue that the legacy transparent private key is derived an extra level to the external scope.
        if version < 29 {
            if let Some(mnemonic) = mnemonic.as_ref() {
                wallet_capability.unified_key_store = UnifiedKeyStore::Spend(Box::new(
                    UnifiedSpendingKey::from_seed(
                        &network,
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
                ));
            } else if let UnifiedKeyStore::Spend(_) = &wallet_capability.unified_key_store {
                return Err(io::Error::new(
                    ErrorKind::Other,
                    "loading from legacy spending keys with no seed phrase to recover",
                ));
            }
        }

        let unified_key_store = wallet_capability.unified_key_store;

        info!("Keys in this wallet:");
        match &unified_key_store {
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

        // FIXME: sync integration, add locators for targetted rescan

        let lw = Self {
            mnemonic,
            wallet_options: Arc::new(RwLock::new(wallet_options)),
            birthday,
            unified_key_store,
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            price: Arc::new(RwLock::new(price)),
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: pepper_sync::wallet::NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: pepper_sync::wallet::ShardTrees::new(),
            sync_state: pepper_sync::wallet::SyncState::new(),
            transparent_addresses: BTreeMap::new(),
            unified_addresses: AppendOnlyVec::new(),
            network,
        };

        Ok(lw)
    }

    fn read_v32<R: Read>(mut reader: R, network: ChainType) -> io::Result<Self> {
        todo!()
    }
}

#[cfg(any(test, feature = "test-elevation"))]
pub mod testing;

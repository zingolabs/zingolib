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

    /// TODO: Add Doc Comment Here!
    // FIXME: sync integration, write rest of wallet data
    pub async fn write<W: Write>(
        &mut self,
        mut writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> io::Result<()> {
        // TODO: version can be u32 (or u16?)
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;
        self.unified_key_store.write(&mut writer, self.network)?;

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

        utils::write_string(&mut writer, &self.network.to_string())?;
        self.wallet_options.read().await.write(&mut writer)?;
        // TODO: birthday can be u32
        writer.write_u64::<LittleEndian>(self.birthday.into())?;
        self.price.read().await.write(&mut writer)?;

        // TODO: consider writing mnemonic before keys
        let seed_bytes = match &self.mnemonic {
            Some(m) => m.0.clone().into_entropy(),
            None => vec![],
        };
        Vector::write(&mut writer, &seed_bytes, |w, byte| w.write_u8(*byte))?;

        if let Some(m) = &self.mnemonic {
            writer.write_u32::<LittleEndian>(m.1)?;
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
        self.sync_state.write(&mut writer)
    }

    /// This is a Wallet constructor.  It is the internal function called by 2 LightWallet
    /// read procedures, by reducing its visibility we constrain possible uses.
    /// Each type that can be deserialized has an associated serialization version.  Our
    /// convention is to omit the type e.g. "wallet" from the local variable ident, and
    /// make explicit (via ident) which variable refers to a value deserialized from
    /// some source ("external") and which is represented as a source-code constant
    /// ("internal").
    pub async fn read_internal<R: Read>(mut reader: R, network: ChainType) -> io::Result<Self> {
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

        let mut wallet_capability = None;
        let mut _transactions = None;
        if external_version < 31 {
            wallet_capability = Some(WalletCapability::read(&mut reader, network)?);

            let mut _blocks = Vector::read(&mut reader, |r| BlockData::read(r))?;

            _transactions = if external_version <= 14 {
                Some(TxMap::read_old(
                    &mut reader,
                    wallet_capability
                        .as_ref()
                        .expect("wallet capability should exist for versions pre-31"),
                )?)
            } else {
                Some(TxMap::read(
                    &mut reader,
                    wallet_capability
                        .as_ref()
                        .expect("wallet capability should exist for versions pre-31"),
                )?)
            };
        }

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

        let wallet_options = if external_version <= 23 {
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

        if external_version <= 22 {
            let _sapling_tree_verified = if external_version <= 12 {
                true
            } else {
                reader.read_u8()? == 1
            };
        }

        if external_version < 31 {
            let _verified_tree = if external_version <= 21 {
                None
            } else {
                Optional::read(&mut reader, |r| {
                    use prost::Message;

                    let buf = Vector::read(r, |r| r.read_u8())?;
                    TreeState::decode(&buf[..])
                        .map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))
                })?
            };
        }

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

        // Derive unified spending key from seed and override temporary USK if wallet is pre v29.
        //
        // UnifiedSpendingKey is initially incomplete for old wallet versions.
        // This is due to the legacy transparent extended private key (ExtendedPrivKey) not containing all information required for BIP0032.
        // There is also the issue that the legacy transparent private key is derived an extra level to the external scope.
        if external_version < 29 {
            if let Some(mnemonic) = mnemonic.as_ref() {
                wallet_capability
                    .as_mut()
                    .expect("wallet capability should exist for versions pre-31")
                    .unified_key_store = UnifiedKeyStore::Spend(Box::new(
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
            } else if let UnifiedKeyStore::Spend(_) = &wallet_capability
                .as_ref()
                .expect("wallet capability should exist for versions pre-31")
                .unified_key_store
            {
                return Err(io::Error::new(
                    ErrorKind::Other,
                    "loading from legacy spending keys with no seed phrase to recover",
                ));
            }
        }

        let unified_key_store = if external_version >= 31 {
            UnifiedKeyStore::read(&mut reader, network)?
            // FIXME: sync integration, check write matches read for v31
        } else {
            wallet_capability
                .expect("wallet capability should exist for versions pre-31")
                .unified_key_store
        };

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

        if external_version >= 31 {
            // FIXME: sync integration, load new wallet format
        } else {
            // FIXME: sync integration, add locators for targetted rescan
        }

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
}

#[cfg(any(test, feature = "test-elevation"))]
pub mod testing;

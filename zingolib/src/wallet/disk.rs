//! This mod contains write and read functionality of impl LightWallet
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use log::info;
use pepper_sync::{
    keys::transparent::{self, TransparentAddressId, TransparentScope},
    wallet::{NullifierMap, OutputId, ShardTrees, SyncState, WalletBlock, WalletTransaction},
};
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

use zcash_primitives::{
    consensus::{self, BlockHeight},
    transaction::TxId,
};

use crate::{config::ChainType, wallet::keys::unified::UnifiedKeyStore};

use crate::wallet::traits::ReadableWriteable;
use crate::wallet::WalletOptions;
use crate::wallet::{utils, SendProgress};

use super::keys::unified::{ReceiverSelection, UnifiedAddressId, WalletCapability};

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
            |w, (address_id, address)| {
                w.write_u32::<LittleEndian>(address_id.account_id.into())?;
                w.write_u32::<LittleEndian>(address_id.address_index)?;
                ReceiverSelection {
                    orchard: address.orchard().is_some(),
                    sapling: address.sapling().is_some(),
                    transparent: address.transparent().is_some(),
                }
                .write(w, ())
            },
        )?;
        Vector::write(
            &mut writer,
            &self.transparent_addresses.keys().collect::<Vec<_>>(),
            |w, address_id| {
                w.write_u32::<LittleEndian>(address_id.account_id().into())?;
                w.write_u8(address_id.scope() as u8)?;
                w.write_u32::<LittleEndian>(address_id.address_index())
            },
        )?;

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
    // TODO: update to return WalletError
    pub fn read<R: Read>(mut reader: R, network: ChainType) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        info!("Reading wallet version {}", version);
        match version {
            ..32 => Self::read_v0(reader, network, version),
            32 => Self::read_v32(reader, network),
            _ => Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Failed to read wallet version {}. Do you have the latest version?\n{}",
                    version, "Note: wallet files from zecwallet or beta zingo are not compatible"
                ),
            )),
        }
    }

    fn read_v0<R: Read>(mut reader: R, network: ChainType, version: u64) -> io::Result<Self> {
        let mut wallet_capability = WalletCapability::read(&mut reader, network)?;
        let mut _blocks = Vector::read(&mut reader, |r| BlockData::read(r))?;
        let transactions = if version <= 14 {
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

        // setup targetted scanning from zingo 1.x transaction data
        let mut sync_state = SyncState::new();
        pepper_sync::add_scan_targets(
            &mut sync_state,
            &transactions
                .transaction_records_by_id
                .values()
                .filter_map(|transaction| {
                    transaction
                        .status
                        .get_confirmed_height()
                        .map(|height| (height, transaction.txid))
                })
                .collect::<Vec<_>>(),
        );

        let lw = Self {
            mnemonic,
            wallet_options: Arc::new(RwLock::new(wallet_options)),
            birthday,
            unified_key_store,
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            price: Arc::new(RwLock::new(price)),
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: ShardTrees::new(),
            sync_state,
            transparent_addresses: BTreeMap::new(),
            unified_addresses: BTreeMap::new(),
            network,
            save_required: false,
        };

        Ok(lw)
    }

    fn read_v32<R: Read>(mut reader: R, network: ChainType) -> io::Result<Self> {
        let saved_network = utils::read_string(&mut reader)?;
        if saved_network != network.to_string() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Wallet chain name {} doesn't match expected {}",
                    saved_network, network
                ),
            ));
        }

        let seed_bytes = Vector::read(&mut reader, |r| r.read_u8())?;
        let mnemonic = if !seed_bytes.is_empty() {
            let account_index = reader.read_u32::<LittleEndian>()?;
            Some((
                <Mnemonic>::from_entropy(seed_bytes)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?,
                account_index,
            ))
        } else {
            None
        };
        let birthday = BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?);
        let unified_key_store = UnifiedKeyStore::read(&mut reader, network)?;

        let unified_addresses = Vector::read(&mut reader, |r| {
            let account_id = zip32::AccountId::try_from(r.read_u32::<LittleEndian>()?)
                .expect("only valid account ids are stored");
            let address_index = r.read_u32::<LittleEndian>()?;
            let receivers = ReceiverSelection::read(r, ())?;

            Ok((
                UnifiedAddressId {
                    account_id,
                    address_index,
                },
                unified_key_store
                    .generate_unified_address(address_index, receivers, false)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            ))
        })?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let transparent_addresses = Vector::read(&mut reader, |r| {
            let account_id = zip32::AccountId::try_from(r.read_u32::<LittleEndian>()?)
                .expect("only valid account ids are stored");
            let scope = TransparentScope::try_from(r.read_u8()?)?;
            let address_index = r.read_u32::<LittleEndian>()?;

            Ok((
                TransparentAddressId::new(account_id, scope, address_index),
                transparent::encode_address(
                    &network,
                    unified_key_store
                        .generate_transparent_address(address_index, scope, false)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                ),
            ))
        })?
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let wallet_blocks = Vector::read(&mut reader, |r| WalletBlock::read(r))?
            .into_iter()
            .map(|block| (block.block_height(), block))
            .collect::<BTreeMap<_, _>>();
        let wallet_transactions =
            Vector::read(&mut reader, |r| WalletTransaction::read(r, &network))?
                .into_iter()
                .map(|transaction| (transaction.txid(), transaction))
                .collect::<HashMap<_, _>>();
        let nullifier_map = NullifierMap::read(&mut reader)?;
        let outpoint_map = Vector::read(&mut reader, |mut r| {
            let outpoint_txid = TxId::read(&mut r)?;
            let output_index = r.read_u16::<LittleEndian>()?;
            let locator_height = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
            let locator_txid = TxId::read(&mut r)?;

            Ok((
                OutputId::new(outpoint_txid, output_index),
                (locator_height, locator_txid),
            ))
        })?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let shard_trees = ShardTrees::read(&mut reader)?;
        let sync_state = SyncState::read(&mut reader)?;

        let wallet_options = WalletOptions::read(&mut reader)?;
        let price = WalletZecPriceInfo::read(&mut reader)?;

        Ok(Self {
            network,
            mnemonic,
            birthday,
            unified_key_store,
            unified_addresses,
            transparent_addresses,
            wallet_blocks,
            wallet_transactions,
            nullifier_map,
            outpoint_map,
            shard_trees,
            sync_state,
            wallet_options: Arc::new(RwLock::new(wallet_options)),
            price: Arc::new(RwLock::new(price)),
            send_progress: Arc::new(RwLock::new(SendProgress::new(0))),
            save_required: false,
        })
    }
}

#[cfg(any(test, feature = "test-elevation"))]
pub mod testing;

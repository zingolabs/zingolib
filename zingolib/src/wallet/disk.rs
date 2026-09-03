//! This mod contains write and read functionality of impl `LightWallet`

use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Error, ErrorKind, Read, Write},
    num::NonZeroU32,
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use log::info;

use bip0039::Mnemonic;
use zip32::AccountId;

use zcash_encoding::{CompactSize, Optional, Vector};
use zcash_keys::keys::UnifiedSpendingKey;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::{self, BlockHeight};
use zcash_transparent::keys::NonHardenedChildIndex;

use zingo_common_components::protocol::ActivationHeights;
use zingo_netutils::lightwallet_protocol::TreeState;
use zingo_price::PriceList;

use super::keys::unified::{ReceiverSelection, UnifiedAddressId};
use super::{LightWallet, RecoveryInfo, error::KeyError};
use crate::wallet::{WalletSettings, legacy::WalletZecPriceInfo, utils};
use crate::wallet::{legacy::WalletOptions, traits::ReadableWriteable};
use crate::{
    config::ChainType,
    wallet::{
        keys::{legacy::WalletCapability, unified::UnifiedKeyStore},
        legacy::{BlockData, TxMap},
    },
};
use pepper_sync::{
    config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery},
    keys::transparent::{self, TransparentAddressId, TransparentScope},
    wallet::{
        KeyIdInterface, NullifierMap, OutputId, ScanTarget, ShardTrees, SyncState, WalletBlock,
        WalletTransaction,
    },
};

/// The two trailing sections of a version 42 wallet file: the price list and
/// the optional Orchard→Ironwood migration section.
type WalletTail = (PriceList, Option<crate::wallet::migration::MigrationState>);

enum V40ChainField {
    Tag(u8),
    Name(String),
}

fn read_v40_chain_field<R: Read>(reader: &mut R) -> io::Result<V40ChainField> {
    let first_byte = reader.read_u8()?;
    if first_byte <= 2 {
        Ok(V40ChainField::Tag(first_byte))
    } else {
        let mut length_bytes = [0u8; 8];
        length_bytes[0] = first_byte;
        reader.read_exact(&mut length_bytes[1..])?;
        Ok(V40ChainField::Name(utils::read_string_body(
            reader,
            u64::from_le_bytes(length_bytes),
        )?))
    }
}

fn chain_type_from_tag(tag: u8) -> io::Result<ChainType> {
    match tag {
        0 => Ok(ChainType::Mainnet),
        1 => Ok(ChainType::Testnet),
        2 => Ok(ChainType::Regtest(ActivationHeights::default())),
        other => Err(Error::new(
            ErrorKind::InvalidData,
            format!("invalid chain type index stored in wallet file: {}", other,),
        )),
    }
}

fn chain_name_from_stored(stored: &str) -> io::Result<&'static str> {
    match stored {
        "main" => Ok("mainnet"),
        "test" => Ok("testnet"),
        "regtest" => Ok("regtest"),
        other => Err(Error::new(
            ErrorKind::InvalidData,
            format!("invalid chain type stored in wallet file: {}", other,),
        )),
    }
}

struct CountingReader<R> {
    inner: R,
    offset: u64,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        self.offset += bytes_read as u64;
        Ok(bytes_read)
    }
}

/// Reads a little-endian u32 and rejects values outside the ZIP 32 account id range as `InvalidData`.
fn read_account_id<R: Read>(reader: &mut R) -> io::Result<zip32::AccountId> {
    let raw_account_id = reader.read_u32::<LittleEndian>()?;
    zip32::AccountId::try_from(raw_account_id).map_err(|_| {
        Error::new(
            ErrorKind::InvalidData,
            format!("invalid account id {raw_account_id} stored in wallet file"),
        )
    })
}

fn check_saved_chain(saved_network: &str, chain_type: &ChainType) -> io::Result<()> {
    if saved_network == chain_type.to_string() {
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::InvalidData,
            format!("wallet chain name {saved_network} doesn't match expected {chain_type}"),
        ))
    }
}

impl LightWallet {
    /// Version 40 was minted once per branch (Format Census, issue #2590,
    /// rows 69 and 70). dev's revision (2026-03-25, `eda1dca85` via
    /// `3f95e4520`) wrote the chain type as a u8 tag and outpoint indices
    /// as u16; stable's revision (`5d8fda797`) kept the chain-name string
    /// and widened outpoint indices to u32. The reader separates the two
    /// grammars on the byte after the version word: a chain tag is 0
    /// through 2, and a chain-name string length's low byte is 4 or 7.
    ///
    /// Changes in version 41:
    /// `ChainType` serialized as u8 instead of string to decouple from fmt::Display and reduce bytes stored.
    ///
    /// Changes in version 42:
    /// Optional Orchard→Ironwood migration section appended (see
    /// `crate::wallet::migration::store`. The section carries its own inner
    /// version). (An earlier revision of 42 also wrote an
    /// `allow_v6_transactions` bool after `min_confirmations`. The setting
    /// was later removed and version 42 redefined without the byte, leaving
    /// two shipped layouts under one number. Both are read, disambiguated
    /// via `Self::read_price_and_migration`.)
    ///
    /// Version 43 is burned: builds between the two revisions of 42 wrote
    /// it with the final version 42 layout, so it is accepted at read as 42
    /// and must never be assigned to a new layout. The next format bump is
    /// 44.
    ///
    /// Landing in dev ships a format: every layout that has landed in dev
    /// must remain readable, and the wallet writable, forever after (ADR
    /// 0015, docs/adr/0015-landing-in-dev-ships-the-wallet-file-format.md).
    #[must_use]
    pub const fn serialized_version() -> u64 {
        42
    }

    /// Upper bound on the version word [`Self::read_recovery_info`] accepts:
    /// far above any version this project will reach, low enough that random
    /// or encrypted bytes cannot land in it.
    pub const MAX_RECOVERABLE_VERSION: u64 = 1000;

    /// Upper bound on the birthday [`Self::read_recovery_info`] accepts: no
    /// real chain reaches this height for centuries, while fabricated
    /// prefixes decode to birthdays far above it.
    pub const MAX_RECOVERABLE_BIRTHDAY: u32 = 100_000_000;

    /// Serialize into `writer`
    pub fn write<W: Write>(
        &mut self,
        mut writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;
        writer.write_u8(match self.chain_type() {
            ChainType::Mainnet => 0,
            ChainType::Testnet => 1,
            ChainType::Regtest(_) => 2,
        })?;
        let seed_bytes = match &self.mnemonic {
            Some(m) => m.clone().into_entropy(),
            None => vec![],
        };
        Vector::write(&mut writer, &seed_bytes, |w, byte| w.write_u8(*byte))?;
        writer.write_u32::<LittleEndian>(self.birthday.into())?;
        Vector::write(
            &mut writer,
            &self.unified_key_store.iter().collect::<Vec<_>>(),
            |w, (account_id, unified_key)| {
                w.write_u32::<LittleEndian>(u32::from(**account_id))?;
                unified_key.write(w, self.chain_type)
            },
        )?;
        // TODO: also store receiver selections in encoded memos.
        Vector::write(
            &mut writer,
            &self.unified_addresses.iter().collect::<Vec<_>>(),
            |w, (address_id, address)| {
                w.write_u32::<LittleEndian>(address_id.account_id.into())?;
                w.write_u32::<LittleEndian>(address_id.address_index)?;
                ReceiverSelection {
                    orchard: address.orchard().is_some(),
                    sapling: address.sapling().is_some(),
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
                w.write_u32::<LittleEndian>(address_id.address_index().index())
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
            |w, &(&output_id, &scan_target)| {
                output_id.txid().write(&mut *w)?;
                w.write_u32::<LittleEndian>(output_id.output_index())?;
                scan_target.write(w)
            },
        )?;
        self.shard_trees.write(&mut writer)?;
        self.sync_state.write(&mut writer)?;
        self.wallet_settings.sync_config.write(&mut writer)?;
        writer.write_u32::<LittleEndian>(self.wallet_settings.min_confirmations.into())?;
        self.price_list.write(&mut writer)?;
        Optional::write(&mut writer, self.migration.as_ref(), |w, migration| {
            crate::wallet::migration::store::write(w, migration)
        })
    }

    /// Deserialize into `reader`
    // TODO: update to return WalletError
    pub fn read<R: Read>(mut reader: R, chain_type: ChainType) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        info!("Reading wallet version {version}");
        match version {
            ..32 => Self::read_v0(reader, chain_type, version),
            // 43 is a burned version number with the final 42 layout; see
            // the `serialized_version` docs and ADR 0015.
            32..=43 => Self::read_v32(reader, chain_type, version),
            _ => Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "Failed to read wallet version {}. Do you have the latest version?\n{}",
                    version, "Note: wallet files from zecwallet or beta zingo are not compatible"
                ),
            )),
        }
    }

    /// Confirms the bytes parse as a complete wallet this build can read, by
    /// running the full [`Self::read`] deserialization and discarding the
    /// result; on failure the error names the byte offset reached.
    pub fn validate<R: Read>(reader: R, chain_type: ChainType) -> io::Result<()> {
        let mut counting_reader = CountingReader {
            inner: reader,
            offset: 0,
        };
        Self::read(&mut counting_reader, chain_type).map_err(|error| {
            Error::new(
                error.kind(),
                format!(
                    "wallet file failed to parse at byte {}: {error}",
                    counting_reader.offset
                ),
            )
        })?;
        Ok(())
    }

    fn read_v0<R: Read>(mut reader: R, chain_type: ChainType, version: u64) -> io::Result<Self> {
        let mut wallet_capability = WalletCapability::read(&mut reader, chain_type)?;
        let mut _blocks = Vector::read(&mut reader, |r| BlockData::read(r))?;
        let transactions = if version <= 14 {
            TxMap::read_old(&mut reader, &wallet_capability)?
        } else {
            TxMap::read(&mut reader, &wallet_capability)?
        };

        let saved_network = match utils::read_string(&mut reader)?.as_str() {
            "main" => "mainnet",
            "test" => "testnet",
            "regtest" => "regtest",
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("invalid chain type stored in wallet file: {}", other,),
                ));
            }
        };
        if saved_network != chain_type.to_string() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("wallet chain name {saved_network} doesn't match expected {chain_type}"),
            ));
        }

        let _wallet_options = if version <= 23 {
            WalletOptions::default()
        } else {
            WalletOptions::read(&mut reader)?
        };
        let stored_birthday = reader.read_u64::<LittleEndian>()?;
        let birthday = BlockHeight::from_u32(stored_birthday.try_into().map_err(|_| {
            Error::new(
                ErrorKind::InvalidData,
                format!("stored birthday {stored_birthday} exceeds the maximum block height"),
            )
        })?);

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

                let buf = Vector::read(r, byteorder::ReadBytesExt::read_u8)?;
                TreeState::decode(&buf[..])
                    .map_err(|e| io::Error::new(ErrorKind::InvalidData, e.to_string()))
            })?
        };

        let _price = if version <= 13 {
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

        let seed_bytes = Vector::read(&mut reader, byteorder::ReadBytesExt::read_u8)?;
        let mnemonic = if seed_bytes.is_empty() {
            None
        } else {
            let _account_index = if version >= 28 {
                reader.read_u32::<LittleEndian>()?
            } else {
                0
            };
            Some(
                Mnemonic::from_entropy(seed_bytes)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?,
            )
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
                        &chain_type,
                        &mnemonic.to_seed(""),
                        AccountId::ZERO,
                    )
                    .map_err(|e| {
                        Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "failed to derive unified spending key from stored seed bytes. {e}"
                            ),
                        )
                    })?,
                ));
            } else if let UnifiedKeyStore::Spend(_) = &wallet_capability.unified_key_store {
                return Err(io::Error::other(
                    "loading from legacy spending keys with no seed to recover",
                ));
            }
        }

        let mut unified_key_store = BTreeMap::new();
        unified_key_store.insert(zip32::AccountId::ZERO, wallet_capability.unified_key_store);
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
                .generate_unified_address(unified_address_id.address_index, receivers)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
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
                    transparent::encode_address(&chain_type, first_transparent_address),
                );
            }
            Err(KeyError::NoViewCapability) => (),
            Err(e) => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("failed to create transparent address. {e}"),
                ));
            }
        }

        // setup targetted scanning from zingo 1.x transaction data
        let mut sync_state = SyncState::new();
        pepper_sync::add_scan_targets(
            &mut sync_state,
            &transactions
                .transaction_records_by_id
                .0
                .values()
                .filter_map(|transaction| {
                    transaction
                        .status
                        .get_confirmed_height()
                        .map(|height| ScanTarget {
                            block_height: height,
                            txid: transaction.txid,
                            narrow_scan_area: true,
                        })
                })
                .collect::<Vec<_>>(),
        );

        let lw = Self {
            current_version: LightWallet::serialized_version(),
            read_version: version,
            mnemonic,
            birthday,
            unified_key_store,
            price_list: PriceList::new(),
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: ShardTrees::new(),
            sync_state,
            transparent_addresses,
            unified_addresses,
            chain_type,
            migration: None,
            send_proposal: None,
            output_locks: crate::wallet::locks::OutputLocks::default(),
            save_required: false,
            wallet_settings: WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::default(),
                    performance_level: PerformanceLevel::High,
                    shutdown_on_completion: false,
                },
                min_confirmations: NonZeroU32::try_from(3).unwrap(),
            },
        };

        Ok(lw)
    }

    fn read_v32<R: Read>(mut reader: R, chain_type: ChainType, version: u64) -> io::Result<Self> {
        let dev_v40_grammar = if version >= 41 {
            let saved_network = chain_type_from_tag(reader.read_u8()?)?;
            check_saved_chain(&saved_network.to_string(), &chain_type)?;
            false
        } else if version == 40 {
            match read_v40_chain_field(&mut reader)? {
                V40ChainField::Tag(tag) => {
                    check_saved_chain(&chain_type_from_tag(tag)?.to_string(), &chain_type)?;
                    true
                }
                V40ChainField::Name(stored) => {
                    check_saved_chain(chain_name_from_stored(&stored)?, &chain_type)?;
                    false
                }
            }
        } else {
            let stored = utils::read_string(&mut reader)?;
            check_saved_chain(chain_name_from_stored(&stored)?, &chain_type)?;
            false
        };

        let seed_bytes = Vector::read(&mut reader, byteorder::ReadBytesExt::read_u8)?;
        let mnemonic = if seed_bytes.is_empty() {
            None
        } else {
            if version < 35 {
                let _account_index = reader.read_u32::<LittleEndian>()?;
            }
            Some(
                <Mnemonic>::from_entropy(seed_bytes)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?,
            )
        };
        let birthday = BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?);

        let unified_key_store = if version >= 35 {
            Vector::read(&mut reader, |r| {
                Ok((read_account_id(r)?, UnifiedKeyStore::read(r, chain_type)?))
            })?
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        } else {
            let mut keys = BTreeMap::new();
            keys.insert(
                zip32::AccountId::ZERO,
                UnifiedKeyStore::read(&mut reader, chain_type)?,
            );
            keys
        };

        let mut unified_addresses = Vector::read(&mut reader, |r| {
            let account_id = read_account_id(r)?;
            let address_index = r.read_u32::<LittleEndian>()?;
            let receivers = ReceiverSelection::read(r, ())?;

            Ok((
                UnifiedAddressId {
                    account_id,
                    address_index,
                },
                unified_key_store
                    .get(&account_id)
                    .ok_or(Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "unified addresses found for account {} but was account not found",
                            u32::from(account_id)
                        ),
                    ))?
                    .generate_unified_address(address_index, receivers)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            ))
        })?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let mut transparent_addresses = Vector::read(&mut reader, |r| {
            let account_id = read_account_id(r)?;
            let scope = TransparentScope::try_from(r.read_u8()?)?;
            let raw_address_index = r.read_u32::<LittleEndian>()?;
            let address_index =
                NonHardenedChildIndex::from_index(raw_address_index).ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        format!(
                            "hardened transparent address index {raw_address_index} stored in wallet file"
                        ),
                    )
                })?;

            Ok((
                TransparentAddressId::new(account_id, scope, address_index),
                transparent::encode_address(
                    &chain_type,
                    unified_key_store
                        .get(&account_id)
                        .ok_or(Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "unified addresses found for account {} but was account not found",
                                u32::from(account_id)
                            ),
                        ))?
                        .generate_transparent_address(address_index, scope)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                ),
            ))
        })?
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        // reset zingo 2.0 test version addresses
        if version < 36 {
            let unified_key = unified_key_store
                .get(&zip32::AccountId::ZERO)
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "wallet file stores no key for account 0",
                    )
                })?;
            unified_addresses = BTreeMap::new();
            if let Some(receivers) = unified_key.default_receivers() {
                let unified_address_id = UnifiedAddressId {
                    account_id: zip32::AccountId::ZERO,
                    address_index: 0,
                };
                let first_unified_address = unified_key
                    .generate_unified_address(unified_address_id.address_index, receivers)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                unified_addresses.insert(unified_address_id, first_unified_address.clone());
            }

            transparent_addresses = BTreeMap::new();
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
                        transparent::encode_address(&chain_type, first_transparent_address),
                    );
                }
                Err(KeyError::NoViewCapability) => (),
                Err(e) => {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        format!("failed to create transparent address. {e}"),
                    ));
                }
            }
        }

        let wallet_blocks = Vector::read(&mut reader, |r| WalletBlock::read(r))?
            .into_iter()
            .map(|block| (block.block_height(), block))
            .collect::<BTreeMap<_, _>>();
        let wallet_transactions =
            Vector::read(&mut reader, |r| WalletTransaction::read(r, &chain_type))?
                .into_iter()
                .map(|transaction| (transaction.txid(), transaction))
                .collect::<HashMap<_, _>>();
        let nullifier_map = NullifierMap::read(&mut reader)?;
        let outpoint_map = Vector::read(&mut reader, |mut r| {
            let outpoint_txid = TxId::read(&mut r)?;
            let output_index = if version >= 40 && !dev_v40_grammar {
                r.read_u32::<LittleEndian>()?
            } else {
                u32::from(r.read_u16::<LittleEndian>()?)
            };
            let scan_target = if version >= 37 {
                ScanTarget::read(r)?
            } else {
                let block_height = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
                let txid = TxId::read(&mut r)?;

                ScanTarget {
                    block_height,
                    txid,
                    narrow_scan_area: true,
                }
            };

            Ok((OutputId::new(outpoint_txid, output_index), scan_target))
        })?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let shard_trees = ShardTrees::read(&mut reader)?;
        let sync_state = SyncState::read(&mut reader)?;

        let wallet_settings = if version >= 33 {
            let sync_config = SyncConfig::read(&mut reader)?;
            let min_confirmations = if version >= 38 {
                let stored_min_confirmations = reader.read_u32::<LittleEndian>()?;
                NonZeroU32::try_from(stored_min_confirmations).map_err(|_| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "min_confirmations of zero stored in wallet file",
                    )
                })?
            } else {
                NonZeroU32::try_from(3).expect("hard-coded non-zero integer")
            };
            WalletSettings {
                sync_config,
                min_confirmations,
            }
        } else {
            WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::default(),
                    performance_level: PerformanceLevel::High,
                    shutdown_on_completion: false,
                },
                min_confirmations: NonZeroU32::try_from(3).unwrap(),
            }
        };

        let (price_list, migration) = if version == 42 {
            // Version 42 exists in two layouts: pre-release builds wrote an
            // `allow_v6_transactions` byte here, before the price list (see
            // the `serialized_version` docs). Disambiguate by parsing the
            // buffered tail anchored at end of file, both ways.
            let mut tail = Vec::new();
            reader.read_to_end(&mut tail)?;

            let canonical = Self::read_price_and_migration(&tail);
            // The extra byte of the pre-release layout is a bool, so only 0
            // or 1 can begin one; any other leading byte rules that layout
            // out without a second parse.
            let pre_release = match tail.first() {
                Some(0 | 1) => Some(Self::read_price_and_migration(&tail[1..])),
                _ => None,
            };

            Self::resolve_v42_tail(canonical, pre_release)?
        } else {
            let price_list = if version >= 34 {
                PriceList::read(&mut reader)?
            } else {
                PriceList::new()
            };

            let migration = if version >= 42 {
                Optional::read(&mut reader, crate::wallet::migration::store::read)?
            } else {
                None
            };

            (price_list, migration)
        };

        Ok(Self {
            current_version: LightWallet::serialized_version(),
            read_version: version,
            chain_type,
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
            wallet_settings,
            price_list,
            migration,
            send_proposal: None,
            output_locks: crate::wallet::locks::OutputLocks::default(),
            save_required: false,
        })
    }

    /// Chooses between the two readings of a version 42 file tail.
    ///
    /// The pre-release reading is consulted only when the canonical reading
    /// fails to parse, so a file written by current code never loads through
    /// the fallback. When both readings parse cleanly the file is genuinely
    /// ambiguous and the load fails rather than guessing: a wrong guess here
    /// would silently substitute a different price list and migration state,
    /// and the seed remains recoverable through [`Self::read_recovery_info`].
    ///
    /// Parity rules out the readings both being migration-free, not the
    /// refusal itself: a migration-free tail is 5 bytes plus a sum of
    /// even-sized optional fields (4- and 8-byte values, and `CompactSize`
    /// widths that grow by 2, 4, or 8), so its length is always odd, while
    /// the two readings differ in length by exactly one. The refusal below
    /// is therefore reachable only when at least one reading carries a
    /// migration section, whose length parity is unconstrained.
    fn resolve_v42_tail(
        canonical: io::Result<WalletTail>,
        pre_release: Option<io::Result<WalletTail>>,
    ) -> io::Result<WalletTail> {
        match (canonical, pre_release) {
            (Ok(_), Some(Ok(_))) => Err(Error::new(
                ErrorKind::InvalidData,
                "ambiguous version 42 wallet file: its tail parses as both the \
                 canonical and the pre-release layout, so which build wrote it \
                 cannot be determined; recover the seed with recovery_info",
            )),
            (Ok(parsed), _) => Ok(parsed),
            (Err(_), Some(Ok(parsed))) => Ok(parsed),
            (Err(canonical_error), _) => Err(canonical_error),
        }
    }

    /// Parses the final section of a version 42 wallet file (the price list
    /// followed by the optional migration section) anchored at end of file.
    /// Trailing bytes are an error, which is what lets the two revisions of
    /// version 42 be told apart: the pre-release revision carries exactly one
    /// extra leading byte, so the two readings start one byte apart while
    /// both must consume the tail exactly to EOF to be accepted.
    fn read_price_and_migration(mut tail: &[u8]) -> io::Result<WalletTail> {
        let price_list = PriceList::read(&mut tail)?;
        let migration = Optional::read(&mut tail, crate::wallet::migration::store::read)?;
        if !tail.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "unexpected trailing bytes after wallet data",
            ));
        }
        Ok((price_list, migration))
    }

    /// Recovers the seed phrase, birthday, and account count from the stable
    /// prefix of a version 32+ wallet file, without parsing the rest of the
    /// file. This is the escape hatch when [`Self::read`] fails on a file
    /// written by an orphaned or unknown format revision: the recovered
    /// info suffices to restore the wallet from seed and rescan.
    ///
    /// Fails on legacy files (version below 32), whose seed is stored too
    /// deep in the file to reach without a full parse, and on view-only
    /// wallets, which store no seed.
    ///
    /// Rejects versions above [`Self::MAX_RECOVERABLE_VERSION`] and birthdays
    /// above [`Self::MAX_RECOVERABLE_BIRTHDAY`], so random or encrypted bytes
    /// cannot come back as a confident seed phrase.
    pub fn read_recovery_info<R: Read>(mut reader: R) -> io::Result<RecoveryInfo> {
        let version = reader.read_u64::<LittleEndian>()?;
        if version < 32 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("wallet version {version} predates the recoverable prefix layout"),
            ));
        }
        if version > Self::MAX_RECOVERABLE_VERSION {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "version {version} is above {}, so this is not a wallet file \
                     this project or any future revision of it could have written",
                    Self::MAX_RECOVERABLE_VERSION
                ),
            ));
        }
        if version >= 41 {
            let _chain_type_index = reader.read_u8()?;
        } else if version == 40 {
            let _chain_field = read_v40_chain_field(&mut reader)?;
        } else {
            let _chain_name = utils::read_string(&mut reader)?;
        }
        let seed_bytes = Vector::read(&mut reader, byteorder::ReadBytesExt::read_u8)?;
        if seed_bytes.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "wallet file stores no seed (view-only wallet); nothing to recover",
            ));
        }
        if version < 35 {
            let _account_index = reader.read_u32::<LittleEndian>()?;
        }
        let mnemonic = <Mnemonic>::from_entropy(seed_bytes)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e.to_string()))?;
        let birthday = reader.read_u32::<LittleEndian>()?;
        if birthday > Self::MAX_RECOVERABLE_BIRTHDAY {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "recovered birthday {birthday} is above block height {}, which no \
                     real wallet can reach; this is not a wallet file",
                    Self::MAX_RECOVERABLE_BIRTHDAY
                ),
            ));
        }
        let no_of_accounts = if version >= 35 {
            u32::try_from(CompactSize::read(&mut reader)?).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidData,
                    format!("stored account count is not a valid u32: {e}"),
                )
            })?
        } else {
            1
        };

        Ok(RecoveryInfo {
            seed_phrase: mnemonic.phrase().to_string(),
            birthday: u64::from(birthday),
            no_of_accounts,
        })
    }
}

#[cfg(any(test, feature = "testutils"))]
pub mod testing;

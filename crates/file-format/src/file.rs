use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Error, ErrorKind, Read, Write},
    num::NonZeroU32,
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use log::info;

use bip0039::Mnemonic;

use zcash_encoding::{CompactSize, Optional, Vector};
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::{self, BlockHeight};
use zcash_transparent::keys::NonHardenedChildIndex;

use zingo_common_components::protocol::ActivationHeights;
use zl_common::price::PriceList;

use crate::{
    MigrationSection, RecoveryInfo, WalletFile, WalletFileParts, WalletFileRef, WalletSettings,
    strings,
};
use zl_common::{
    chain::ChainType,
    config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery},
    keys::{
        KeyError, ReceiverSelection, UnifiedAddressId, UnifiedKeyStore,
        transparent::{self, TransparentAddressId, TransparentScope},
    },
    serialization::ReadableWriteable,
    sync::SyncState,
    wallet::{
        KeyIdInterface, NullifierMap, OutputId, ScanTarget, ShardTrees, TransactionRecord,
        WalletBlock,
    },
};
#[cfg(feature = "legacy")]
use {
    crate::RawMigration,
    crate::legacy::{
        self, BlockData, TxMap, WalletOptions, WalletZecPriceInfo, keys::WalletCapability,
    },
    zcash_keys::keys::UnifiedSpendingKey,
    zip32::AccountId,
};

/// The two trailing sections of a version 42 wallet file: the price list and
/// the optional Orchard→Ironwood migration section.
type WalletTail<M> = (PriceList, Option<M>);

#[derive(Debug)]
enum V42Tail<M> {
    Canonical(WalletTail<M>),
    PreRelease(WalletTail<M>),
    Ambiguous,
    Unreadable(UnreadableV42Tail),
}

impl<M> V42Tail<M> {
    fn from_readings(
        canonical: io::Result<WalletTail<M>>,
        pre_release: io::Result<WalletTail<M>>,
    ) -> Self {
        match (canonical, pre_release) {
            (Ok(_), Ok(_)) => Self::Ambiguous,
            (Ok(canonical), Err(_)) => Self::Canonical(canonical),
            (Err(_), Ok(pre_release)) => Self::PreRelease(pre_release),
            (Err(canonical), Err(pre_release)) => Self::Unreadable(UnreadableV42Tail {
                canonical,
                pre_release,
            }),
        }
    }

    fn into_tail(self) -> io::Result<WalletTail<M>> {
        match self {
            Self::Canonical(tail) | Self::PreRelease(tail) => Ok(tail),
            Self::Ambiguous => Err(Error::new(ErrorKind::InvalidData, AmbiguousV42Tail)),
            Self::Unreadable(unreadable) => {
                Err(Error::new(unreadable.canonical.kind(), unreadable))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V42Layout {
    Canonical,
    PreRelease,
}

#[derive(Debug)]
pub struct AmbiguousV42Tail;

impl std::fmt::Display for AmbiguousV42Tail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "ambiguous version 42 wallet file: its tail parses as both the \
             canonical and the pre-release layout, so which build wrote it \
             cannot be determined; read it with read_with_v42_layout, or \
             recover the seed with recovery_info",
        )
    }
}

impl std::error::Error for AmbiguousV42Tail {}

#[derive(Debug, thiserror::Error)]
#[error("{canonical}; read as the pre-release version 42 layout: {pre_release}")]
pub struct UnreadableV42Tail {
    #[source]
    pub canonical: Error,
    pub pre_release: Error,
}

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
        Ok(V40ChainField::Name(strings::read_string_body(
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

/// A wallet file written for one chain, read for another. It travels inside
/// the `io::Error` the read returns, so a caller can tell this failure apart
/// without reading the message.
#[derive(Debug)]
pub struct ChainMismatch {
    /// The chain the file was written for.
    pub saved: String,
    /// The chain the file was read for.
    pub expected: String,
}

impl std::fmt::Display for ChainMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "wallet chain name {} doesn't match expected {}",
            self.saved, self.expected
        )
    }
}

impl std::error::Error for ChainMismatch {}

fn chain_mismatch(saved_network: &str, chain_type: &ChainType) -> Error {
    Error::new(
        ErrorKind::InvalidData,
        ChainMismatch {
            saved: saved_network.to_string(),
            expected: chain_type.to_string(),
        },
    )
}

fn check_saved_chain(saved_network: &str, chain_type: &ChainType) -> io::Result<()> {
    if saved_network == chain_type.to_string() {
        Ok(())
    } else {
        Err(chain_mismatch(saved_network, chain_type))
    }
}

/// The chain a stored chain name stands for, through the tag of that chain.
fn chain_type_from_stored(stored: &str) -> io::Result<ChainType> {
    chain_type_from_tag(match chain_name_from_stored(stored)? {
        "mainnet" => 0,
        "testnet" => 1,
        _ => 2,
    })
}

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
/// [`MigrationSection`]. The section carries its own inner
/// version). (An earlier revision of 42 also wrote an
/// `allow_v6_transactions` bool after `min_confirmations`. The setting
/// was later removed and version 42 redefined without the byte, leaving
/// two shipped layouts under one number. Both are read, disambiguated
/// via `WalletFile::read_price_and_migration`.)
///
/// Version 43 is burned: builds between the two revisions of 42 wrote
/// it with the final version 42 layout, so it is accepted at read as 42
/// and must never be assigned to a new layout.
///
/// Changes in version 44:
/// `SyncConfig` serialized version 2, which appends the
/// `shutdown_on_completion` bool.
///
/// Changes in version 45:
/// `SyncState` serialized version 5, which appends the transparent scan
/// floor.
/// The readers of the zl-common types
/// embedded in the wallet file refuse a serialized version above the one
/// they write. Builds before version 45 read those types at any version,
/// so 44 and 45 were each minted for a change to one of them. A build that
/// reads version 45 refuses a newer embedded layout by itself, so a change
/// to an embedded type's serialized version takes no new Wallet Version.
///
/// Landing in dev ships a format: every layout that has landed in dev
/// must remain readable, and the wallet writable, forever after.
#[must_use]
pub const fn serialized_version() -> u64 {
    45
}

/// Upper bound on the version word [`read_recovery_info`] accepts:
/// far above any version this project will reach, low enough that random
/// or encrypted bytes cannot land in it.
pub const MAX_RECOVERABLE_VERSION: u64 = 1000;

/// Upper bound on the birthday [`read_recovery_info`] accepts: no
/// real chain reaches this height for centuries, while fabricated
/// prefixes decode to birthdays far above it.
pub const MAX_RECOVERABLE_BIRTHDAY: u32 = 100_000_000;

impl<M: MigrationSection> WalletFileRef<'_, M> {
    /// Serialize into `writer`
    pub fn write<W: Write>(
        &mut self,
        mut writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(serialized_version())?;
        writer.write_u8(match self.chain_type {
            ChainType::Mainnet => 0,
            ChainType::Testnet => 1,
            ChainType::Regtest(_) => 2,
        })?;
        let seed_bytes = match self.mnemonic {
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
        Optional::write(&mut writer, self.migration, |w, migration| {
            migration.write(w)
        })
    }
}

#[cfg(not(feature = "legacy"))]
fn legacy_feature_missing(version: u64) -> Error {
    Error::new(
        ErrorKind::InvalidData,
        format!("wallet version {version} is read only by the `legacy` feature of zl-file-format"),
    )
}

/// The chain a wallet file was written for. Version 32 and later store
/// it right after the version word; older files store it past the keys,
/// so they are read whole under each chain until one parses.
pub fn read_chain(bytes: &[u8]) -> io::Result<ChainType> {
    let mut reader = bytes;
    let version = reader.read_u64::<LittleEndian>()?;
    match version {
        #[cfg(not(feature = "legacy"))]
        ..32 => Err(legacy_feature_missing(version)),
        #[cfg(feature = "legacy")]
        ..32 => [
            ChainType::Mainnet,
            ChainType::Testnet,
            ChainType::Regtest(ActivationHeights::default()),
        ]
        .into_iter()
        .find(|chain| WalletFile::<RawMigration>::read(bytes, *chain).is_ok())
        .ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidData,
                format!("wallet version {version} parses under no chain"),
            )
        }),
        32..=45 => {
            if version >= 41 {
                chain_type_from_tag(reader.read_u8()?)
            } else if version == 40 {
                match read_v40_chain_field(&mut reader)? {
                    V40ChainField::Tag(tag) => chain_type_from_tag(tag),
                    V40ChainField::Name(stored) => chain_type_from_stored(&stored),
                }
            } else {
                let stored = strings::read_string(&mut reader)?;
                chain_type_from_stored(&stored)
            }
        }
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            format!("wallet version {version} is newer than this build reads"),
        )),
    }
}

impl<M: MigrationSection> WalletFile<M> {
    /// Deserialize into `reader`
    // TODO: update to return WalletError
    pub fn read<R: Read>(reader: R, chain_type: ChainType) -> io::Result<Self> {
        Self::read_resolving_v42(reader, chain_type, None)
    }

    pub fn read_with_v42_layout<R: Read>(
        reader: R,
        chain_type: ChainType,
        layout: V42Layout,
    ) -> io::Result<Self> {
        Self::read_resolving_v42(reader, chain_type, Some(layout))
    }

    fn read_resolving_v42<R: Read>(
        mut reader: R,
        chain_type: ChainType,
        v42_layout: Option<V42Layout>,
    ) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        info!("Reading wallet version {version}");
        match version {
            ..32 => Self::read_v0(reader, chain_type, version),
            // 43 is a burned version number with the final 42 layout; see
            // the `serialized_version` docs and ADR 0015.
            32..=45 => Self::read_v32(reader, chain_type, version, v42_layout),
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

    #[cfg(not(feature = "legacy"))]
    fn read_v0<R: Read>(_reader: R, _chain_type: ChainType, version: u64) -> io::Result<Self> {
        Err(legacy_feature_missing(version))
    }

    #[cfg(feature = "legacy")]
    fn read_v0<R: Read>(mut reader: R, chain_type: ChainType, version: u64) -> io::Result<Self> {
        let mut wallet_capability = WalletCapability::read(&mut reader, chain_type)?;
        let mut _blocks = Vector::read(&mut reader, |r| BlockData::read(r))?;
        let transactions = if version <= 14 {
            TxMap::read_old(&mut reader, &wallet_capability)?
        } else {
            TxMap::read(&mut reader, &wallet_capability)?
        };

        let saved_network = match strings::read_string(&mut reader)?.as_str() {
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
            return Err(chain_mismatch(saved_network, &chain_type));
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
                legacy::TreeStateMessage::decode(&buf[..])
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
        zl_common::sync::add_scan_targets(
            &mut sync_state,
            &transactions
                .transaction_records_by_id
                .0
                .values()
                .filter_map(|transaction| {
                    transaction
                        .status
                        .get_confirmed_height()
                        .map(|height| ScanTarget::new(height, Some(transaction.txid)))
                })
                .collect::<Vec<_>>(),
        );

        let lw = Self(WalletFileParts {
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
            wallet_settings: WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::default(),
                    performance_level: PerformanceLevel::High,
                    shutdown_on_completion: false,
                },
                min_confirmations: NonZeroU32::try_from(3).expect("hard-coded non-zero integer"),
            },
        });

        Ok(lw)
    }

    fn read_v32<R: Read>(
        mut reader: R,
        chain_type: ChainType,
        version: u64,
        v42_layout: Option<V42Layout>,
    ) -> io::Result<Self> {
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
            let stored = strings::read_string(&mut reader)?;
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
            Vector::read(&mut reader, |r| TransactionRecord::read(r, &chain_type))?
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

                ScanTarget::new(block_height, Some(txid))
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
                min_confirmations: NonZeroU32::try_from(3).expect("hard-coded non-zero integer"),
            }
        };

        let (price_list, migration) = if version == 42 {
            // Version 42 exists in two layouts: pre-release builds wrote an
            // `allow_v6_transactions` byte here, before the price list (see
            // the `serialized_version` docs). Disambiguate by parsing the
            // buffered tail anchored at end of file, both ways.
            let mut tail = Vec::new();
            reader.read_to_end(&mut tail)?;

            Self::read_v42_tail_as(&tail, chain_type, v42_layout)?
        } else {
            let price_list = if version >= 34 {
                PriceList::read(&mut reader)?
            } else {
                PriceList::new()
            };

            let migration = if version >= 42 {
                Optional::read(&mut reader, |r| M::read(r, chain_type))?
            } else {
                None
            };

            (price_list, migration)
        };

        Ok(Self(WalletFileParts {
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
        }))
    }

    fn read_v42_tail_as(
        tail: &[u8],
        chain_type: ChainType,
        layout: Option<V42Layout>,
    ) -> io::Result<WalletTail<M>> {
        match layout {
            None => Self::read_v42_tail(tail, chain_type).into_tail(),
            Some(V42Layout::Canonical) => Self::read_price_and_migration(tail, chain_type),
            Some(V42Layout::PreRelease) => Self::read_pre_release_tail(tail, chain_type),
        }
    }

    fn read_v42_tail(tail: &[u8], chain_type: ChainType) -> V42Tail<M> {
        V42Tail::from_readings(
            Self::read_price_and_migration(tail, chain_type),
            Self::read_pre_release_tail(tail, chain_type),
        )
    }

    fn read_pre_release_tail(tail: &[u8], chain_type: ChainType) -> io::Result<WalletTail<M>> {
        match tail.split_first() {
            Some((allow_v6_transactions, rest)) if *allow_v6_transactions <= u8::from(true) => {
                Self::read_price_and_migration(rest, chain_type)
            }
            _ => Err(Error::new(
                ErrorKind::InvalidData,
                "the tail does not begin with the bool of the pre-release version 42 layout",
            )),
        }
    }

    /// Parses the final section of a version 42 wallet file (the price list
    /// followed by the optional migration section) anchored at end of file.
    /// Trailing bytes are an error, which is what lets the two revisions of
    /// version 42 be told apart: the pre-release revision carries exactly one
    /// extra leading byte, so the two readings start one byte apart while
    /// both must consume the tail exactly to EOF to be accepted.
    fn read_price_and_migration(
        mut tail: &[u8],
        chain_type: ChainType,
    ) -> io::Result<WalletTail<M>> {
        let price_list = PriceList::read(&mut tail)?;
        let migration = Optional::read(&mut tail, |r| M::read(r, chain_type))?;
        if !tail.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "unexpected trailing bytes after wallet data",
            ));
        }
        Ok((price_list, migration))
    }
}

/// Recovers the seed phrase, birthday, and account count from the stable
/// prefix of a version 32+ wallet file, without parsing the rest of the
/// file. This is the escape hatch when [`WalletFile::read`] fails on a file
/// written by an orphaned or unknown format revision: the recovered
/// info suffices to restore the wallet from seed and rescan.
///
/// Fails on legacy files (version below 32), whose seed is stored too
/// deep in the file to reach without a full parse, and on view-only
/// wallets, which store no seed.
///
/// Rejects versions above [`MAX_RECOVERABLE_VERSION`] and birthdays
/// above [`MAX_RECOVERABLE_BIRTHDAY`], so random or encrypted bytes
/// cannot come back as a confident seed phrase.
pub fn read_recovery_info<R: Read>(mut reader: R) -> io::Result<RecoveryInfo> {
    let version = reader.read_u64::<LittleEndian>()?;
    if version < 32 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("wallet version {version} predates the recoverable prefix layout"),
        ));
    }
    if version > MAX_RECOVERABLE_VERSION {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "version {version} is above {}, so this is not a wallet file \
                 this project or any future revision of it could have written",
                MAX_RECOVERABLE_VERSION
            ),
        ));
    }
    if version >= 41 {
        let _chain_type_index = reader.read_u8()?;
    } else if version == 40 {
        let _chain_field = read_v40_chain_field(&mut reader)?;
    } else {
        let _chain_name = strings::read_string(&mut reader)?;
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
    if birthday > MAX_RECOVERABLE_BIRTHDAY {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "recovered birthday {birthday} is above block height {}, which no \
                 real wallet can reach; this is not a wallet file",
                MAX_RECOVERABLE_BIRTHDAY
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
        birthday_adjusted_from: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{AmbiguousV42Tail, UnreadableV42Tail, V42Layout, V42Tail, chain_mismatch};
    use crate::ChainMismatch;
    use crate::{RawMigration, WalletFile};
    use std::{error::Error as _, io};
    use zl_common::{chain::ChainType, price::PriceList};

    const ALLOW_V6_TRANSACTIONS_FALSE: u8 = 0;
    const PRICE_LIST_VERSION: u8 = 0;
    const NONE: u8 = 0;
    const SOME: u8 = 1;
    const NO_DAILY_PRICES: u8 = 0;
    const ONE_DAILY_PRICE: u8 = 1;
    const PRICE_TIME: [u8; 4] = [0; 4];
    const DAILY_PRICE_ENTRY: [u8; 8] = [0; 8];
    const SECTION: [u8; 3] = [7, 8, 9];

    fn read_raw_tail(tail: &[u8]) -> V42Tail<RawMigration> {
        WalletFile::<RawMigration>::read_v42_tail(tail, ChainType::Mainnet)
    }

    fn price_list_with_a_stored_time() -> Vec<u8> {
        let mut bytes = vec![PRICE_LIST_VERSION, SOME];
        bytes.extend(PRICE_TIME);
        bytes.extend([NONE, NO_DAILY_PRICES]);
        bytes
    }

    #[test]
    fn a_raw_section_reads_a_canonical_version_42_tail_that_parses_one_way() {
        let mut tail = price_list_with_a_stored_time();
        tail.push(SOME);
        tail.extend(SECTION);

        let V42Tail::Canonical((price_list, migration)) = read_raw_tail(&tail) else {
            panic!("the tail parses in the canonical layout alone");
        };
        assert!(price_list.time_historical_prices_last_updated().is_some());
        assert_eq!(migration, Some(RawMigration(SECTION.to_vec())));
    }

    #[test]
    fn a_named_layout_reads_a_version_42_tail_that_parses_both_ways() {
        const SECTION_FILLER: u8 = 7;
        let mut section = vec![SECTION_FILLER; DAILY_PRICE_ENTRY.len()];
        section.push(SOME);
        section.push(SECTION_FILLER);

        let mut tail = vec![PRICE_LIST_VERSION, NONE, NONE, NO_DAILY_PRICES, SOME];
        tail.extend(&section);

        let read_as = |layout| {
            WalletFile::<RawMigration>::read_v42_tail_as(&tail, ChainType::Mainnet, layout)
        };

        let refusal = read_as(None).expect_err("an opaque section lets both layouts parse");
        assert!(
            refusal
                .get_ref()
                .is_some_and(|inner| inner.is::<AmbiguousV42Tail>())
        );

        let (price_list, migration) = read_as(Some(V42Layout::Canonical))
            .expect("the tail was built in the canonical layout");
        assert!(price_list.daily_prices().is_empty());
        assert_eq!(migration, Some(RawMigration(section.clone())));

        let (price_list, migration) = read_as(Some(V42Layout::PreRelease))
            .expect("the tail was built to parse in the pre-release layout too");
        assert_eq!(
            price_list.daily_prices().len(),
            usize::from(ONE_DAILY_PRICE)
        );
        assert_eq!(migration, Some(RawMigration(vec![SECTION_FILLER])));
    }

    #[test]
    fn a_raw_section_reads_a_pre_release_version_42_tail_that_parses_one_way() {
        let mut tail = vec![ALLOW_V6_TRANSACTIONS_FALSE];
        tail.extend(price_list_with_a_stored_time());
        tail.push(NONE);

        let V42Tail::PreRelease((price_list, migration)) = read_raw_tail(&tail) else {
            panic!("the tail parses in the pre-release layout alone");
        };
        assert!(price_list.time_historical_prices_last_updated().is_some());
        assert_eq!(migration, None);
    }

    #[test]
    fn a_raw_section_never_reads_a_pre_release_version_42_tail_as_another_price_list() {
        let mut tail = vec![
            ALLOW_V6_TRANSACTIONS_FALSE,
            PRICE_LIST_VERSION,
            NONE,
            NONE,
            ONE_DAILY_PRICE,
        ];
        tail.extend(DAILY_PRICE_ENTRY);
        tail.push(NONE);

        assert!(matches!(read_raw_tail(&tail), V42Tail::Ambiguous));
        if let Ok((price_list, migration)) = read_raw_tail(&tail).into_tail() {
            assert_eq!(
                price_list.daily_prices().len(),
                usize::from(ONE_DAILY_PRICE),
                "a tail that reads must hold the daily price that was written"
            );
            assert_eq!(migration, None, "the tail was written with no section");
        }
    }

    #[test]
    fn the_two_readings_of_a_version_42_tail_resolve_to_one_state_each() {
        let parsed = || Ok((PriceList::new(), None::<RawMigration>));
        let failed = || {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "tail did not parse",
            ))
        };

        assert!(matches!(
            V42Tail::from_readings(parsed(), parsed()),
            V42Tail::Ambiguous
        ));
        assert!(matches!(
            V42Tail::from_readings(parsed(), failed()),
            V42Tail::Canonical(_)
        ));
        assert!(matches!(
            V42Tail::from_readings(failed(), parsed()),
            V42Tail::PreRelease(_)
        ));
        assert!(matches!(
            V42Tail::from_readings(failed(), failed()),
            V42Tail::Unreadable(_)
        ));
    }

    const PRE_RELEASE_FAILURE: &str = "tail did not parse";

    fn refusal_of_both_readings(canonical: io::Error) -> io::Error {
        let pre_release = io::Error::other(PRE_RELEASE_FAILURE);
        V42Tail::<RawMigration>::from_readings(Err(canonical), Err(pre_release))
            .into_tail()
            .expect_err("a tail that parses in neither layout is refused")
    }

    #[test]
    fn an_unreadable_version_42_tail_keeps_the_error_of_each_reading() {
        let canonical = chain_mismatch("mainnet", &ChainType::Testnet);
        let canonical_kind = canonical.kind();

        let refusal = refusal_of_both_readings(canonical);

        assert_eq!(refusal.kind(), canonical_kind);
        let unreadable = refusal
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<UnreadableV42Tail>())
            .expect("into_tail wraps the two errors in an UnreadableV42Tail");
        assert!(
            unreadable
                .canonical
                .get_ref()
                .is_some_and(|inner| inner.is::<ChainMismatch>())
        );
        assert_eq!(unreadable.pre_release.to_string(), PRE_RELEASE_FAILURE);
    }

    #[test]
    fn the_source_of_an_unreadable_version_42_tail_is_the_canonical_error() {
        let refusal = refusal_of_both_readings(chain_mismatch("mainnet", &ChainType::Testnet));

        let canonical_cause = refusal
            .source()
            .and_then(|source| source.downcast_ref::<io::Error>())
            .and_then(io::Error::get_ref);
        assert!(
            canonical_cause.is_some_and(|inner| inner.is::<ChainMismatch>()),
            "a walk of the source chain must reach the cause of the canonical reading"
        );
    }

    #[test]
    fn the_text_of_an_unreadable_version_42_tail_begins_with_its_source() {
        let canonical = chain_mismatch("mainnet", &ChainType::Testnet);
        let canonical_text = canonical.to_string();

        let refusal = refusal_of_both_readings(canonical);

        assert!(
            refusal.to_string().starts_with(&canonical_text),
            "the canonical cause leads the text, and a walker skips a source the text holds: {refusal}"
        );
    }

    #[test]
    fn only_a_version_42_tail_that_parses_one_way_becomes_a_tail() {
        let tail = || (PriceList::new(), None::<RawMigration>);
        let failed = || std::io::Error::new(std::io::ErrorKind::InvalidData, "tail did not parse");

        assert!(V42Tail::Canonical(tail()).into_tail().is_ok());
        assert!(V42Tail::PreRelease(tail()).into_tail().is_ok());
        assert!(V42Tail::<RawMigration>::Ambiguous.into_tail().is_err());
        assert!(
            V42Tail::<RawMigration>::Unreadable(UnreadableV42Tail {
                canonical: failed(),
                pre_release: failed(),
            })
            .into_tail()
            .is_err()
        );
    }

    #[cfg(not(feature = "legacy"))]
    #[test]
    fn a_legacy_file_without_the_legacy_feature_names_the_feature() {
        use zl_common::chain::ChainType;

        let legacy_version: u64 = 31;
        let bytes = legacy_version.to_le_bytes();
        let errors = [
            super::read_chain(&bytes)
                .expect_err("read_chain reads no file below version 32 without the legacy feature"),
            WalletFile::<RawMigration>::read(&bytes[..], ChainType::Mainnet).expect_err(
                "WalletFile::read reads no file below version 32 without the legacy feature",
            ),
        ];
        for error in errors {
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert!(
                error.to_string().contains("`legacy` feature"),
                "the error must name the missing feature: {error}"
            );
        }
    }
}

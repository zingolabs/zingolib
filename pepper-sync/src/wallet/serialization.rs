//! Serialization and de-serialization of wallet structs in [`crate::wallet`] including utilities.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    mem::size_of,
    ops::Range,
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use incrementalmerkletree::{Hashable, Position};
use shardtree::{
    LocatedPrunableTree, ShardTree,
    store::{Checkpoint, ShardStore, TreeState, memory::MemoryShardStore},
};
use zcash_client_backend::serialization::shardtree::{read_shard, write_shard};
use zcash_encoding::{Optional, Vector};
use zcash_primitives::{
    block::BlockHash,
    merkle_tree::HashSer,
    transaction::{Transaction, TxId},
};
use zcash_protocol::{
    consensus::{self, BlockHeight},
    memo::Memo,
    value::Zatoshis,
};
use zcash_transparent::address::Script;

use zcash_transparent::keys::NonHardenedChildIndex;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::{
    keys::{
        KeyId, decode_unified_address,
        transparent::{TransparentAddressId, TransparentScope},
    },
    sync::{MAX_REORG_ALLOWANCE, ScanPriority, ScanRange},
    wallet::ScanTarget,
};

use super::{
    InitialSyncState, IronwoodNote, KeyIdInterface, NullifierMap, OrchardNote,
    OutgoingIronwoodNote, OutgoingNote, OutgoingNoteInterface, OutgoingOrchardNote,
    OutgoingSaplingNote, OutputId, OutputInterface, SaplingNote, ShardTrees, SyncState,
    TransparentCoin, TreeBounds, WalletBlock, WalletNote, WalletTransaction,
};

/// Returns `InvalidData` when `version` is newer than the latest layout this build can read.
fn reject_unknown_version(type_name: &str, version: u8, latest: u8) -> std::io::Result<()> {
    if version > latest {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "unknown {type_name} serialization version {version}, latest supported is {latest}"
            ),
        ));
    }
    Ok(())
}

/// Reads one layout version byte, rejecting any version newer than the latest this build can read.
pub(crate) fn read_version<R: Read>(
    mut reader: R,
    type_name: &str,
    latest: u8,
) -> std::io::Result<u8> {
    let version = reader.read_u8()?;
    reject_unknown_version(type_name, version, latest)?;
    Ok(version)
}

/// The byte width of a canonical encoding of a field element on either curve family.
const FIELD_ELEMENT_SIZE: usize = size_of::<<jubjub::Fr as ff::PrimeField>::Repr>();

const _: () = assert!(
    FIELD_ELEMENT_SIZE == size_of::<<pasta_curves::pallas::Base as ff::PrimeField>::Repr>()
);

/// The byte width of a block hash.
const BLOCK_HASH_SIZE: usize = size_of::<BlockHash>();

/// The byte width of a shielded payment address diversifier.
const DIVERSIFIER_SIZE: usize = size_of::<sapling_crypto::keys::Diversifier>();

/// The byte width of a raw sapling or orchard payment address, a diversifier followed by a transmission key.
const RAW_ADDRESS_SIZE: usize = DIVERSIFIER_SIZE + FIELD_ELEMENT_SIZE;

/// The byte width of a serialized memo field, derived as the difference between the full and compact note plaintext widths.
const MEMO_SIZE: usize =
    zcash_note_encryption::NOTE_PLAINTEXT_SIZE - zcash_note_encryption::COMPACT_NOTE_SIZE;

/// Reads exactly `N` bytes from `reader` into a fixed-size array.
fn read_array<const N: usize>(mut reader: impl Read) -> std::io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Converts an empty parse result into an `InvalidData` error naming the corrupt `field`.
fn parse_field<T>(parsed: impl Into<Option<T>>, field: &str) -> std::io::Result<T> {
    parsed.into().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("invalid {field}"))
    })
}

fn read_string<R: Read>(mut reader: R) -> std::io::Result<String> {
    let str_len = reader.read_u64::<LittleEndian>()?;
    let mut str_bytes = Vec::new();
    reader.take(str_len).read_to_end(&mut str_bytes)?;
    if str_bytes.len() as u64 != str_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "string length prefix claims {str_len} bytes but only {} were available",
                str_bytes.len()
            ),
        ));
    }

    String::from_utf8(str_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

fn read_orchard_nullifier<R: Read>(mut reader: R) -> std::io::Result<orchard::note::Nullifier> {
    let nullifier_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut reader)?;
    parse_field(
        orchard::note::Nullifier::from_bytes(&nullifier_bytes),
        "orchard nullifier",
    )
}

fn write_string<W: Write>(mut writer: W, str: &str) -> std::io::Result<()> {
    writer.write_u64::<LittleEndian>(str.len() as u64)?;
    writer.write_all(str.as_bytes())
}

impl ScanTarget {
    fn serialized_version() -> u8 {
        0
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        read_version(&mut reader, "ScanTarget", Self::serialized_version())?;
        let block_height = BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?);
        let txid = TxId::read(&mut reader)?;
        let narrow_scan_area = reader.read_u8()? != 0;

        Ok(Self {
            block_height,
            txid,
            narrow_scan_area,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        writer.write_u32::<LittleEndian>(self.block_height.into())?;
        self.txid.write(&mut *writer)?;
        writer.write_u8(u8::from(self.narrow_scan_area))
    }
}

impl SyncState {
    fn serialized_version() -> u8 {
        // Version 4 inserts the ironwood shard ranges after the orchard ones.
        4
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let version = read_version(&mut reader, "SyncState", Self::serialized_version())?;
        let scan_ranges = Vector::read(&mut reader, |r| {
            let start = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
            let end = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
            let priority = match version {
                3.. => match r.read_u8()? {
                    0 => Ok(ScanPriority::RefetchingNullifiers),
                    1 => Ok(ScanPriority::Scanning),
                    2 => Ok(ScanPriority::Scanned),
                    3 => Ok(ScanPriority::ScannedWithoutMapping),
                    4 => Ok(ScanPriority::Historic),
                    5 => Ok(ScanPriority::OpenAdjacent),
                    6 => Ok(ScanPriority::FoundNote),
                    7 => Ok(ScanPriority::ChainTip),
                    8 => Ok(ScanPriority::Verify),
                    _ => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid scan priority",
                    )),
                }?,
                2 => match r.read_u8()? {
                    0 => Ok(ScanPriority::Scanning),
                    1 => Ok(ScanPriority::Scanned),
                    2 => Ok(ScanPriority::ScannedWithoutMapping),
                    3 => Ok(ScanPriority::Historic),
                    4 => Ok(ScanPriority::OpenAdjacent),
                    5 => Ok(ScanPriority::FoundNote),
                    6 => Ok(ScanPriority::ChainTip),
                    7 => Ok(ScanPriority::Verify),
                    _ => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid scan priority",
                    )),
                }?,
                0 | 1 => match r.read_u8()? {
                    0 => Ok(ScanPriority::Scanning),
                    1 => Ok(ScanPriority::Scanned),
                    2 => Ok(ScanPriority::Historic),
                    3 => Ok(ScanPriority::OpenAdjacent),
                    4 => Ok(ScanPriority::FoundNote),
                    5 => Ok(ScanPriority::ChainTip),
                    6 => Ok(ScanPriority::Verify),
                    _ => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid scan priority",
                    )),
                }?,
            };

            Ok(ScanRange::from_parts(start..end, priority))
        })?;
        let sapling_shard_ranges = Vector::read(&mut reader, |r| {
            let start = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
            let end = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);

            Ok(start..end)
        })?;
        let orchard_shard_ranges = Vector::read(&mut reader, |r| {
            let start = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
            let end = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);

            Ok(start..end)
        })?;
        let ironwood_shard_ranges = if version >= 4 {
            Vector::read(&mut reader, |r| {
                let start = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
                let end = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);

                Ok(start..end)
            })?
        } else {
            Vec::new()
        };
        let scan_targets = Vector::read(&mut reader, |r| {
            Ok(if version >= 1 {
                ScanTarget::read(r)?
            } else {
                let block_height = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
                let txid = TxId::read(r)?;

                ScanTarget {
                    block_height,
                    txid,
                    narrow_scan_area: true,
                }
            })
        })?
        .into_iter()
        .collect::<BTreeSet<_>>();

        Ok(Self {
            scan_ranges,
            sapling_shard_ranges,
            orchard_shard_ranges,
            ironwood_shard_ranges,
            scan_targets,
            initial_sync_state: InitialSyncState::new(),
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&mut self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        Vector::write(&mut writer, self.scan_ranges(), |w, scan_range| {
            w.write_u32::<LittleEndian>(scan_range.block_range().start.into())?;
            w.write_u32::<LittleEndian>(scan_range.block_range().end.into())?;
            w.write_u8(scan_range.priority() as u8)
        })?;
        Vector::write(&mut writer, &self.sapling_shard_ranges, |w, shard_range| {
            w.write_u32::<LittleEndian>(shard_range.start.into())?;
            w.write_u32::<LittleEndian>(shard_range.end.into())
        })?;
        Vector::write(&mut writer, &self.orchard_shard_ranges, |w, shard_range| {
            w.write_u32::<LittleEndian>(shard_range.start.into())?;
            w.write_u32::<LittleEndian>(shard_range.end.into())
        })?;
        Vector::write(
            &mut writer,
            &self.ironwood_shard_ranges,
            |w, shard_range| {
                w.write_u32::<LittleEndian>(shard_range.start.into())?;
                w.write_u32::<LittleEndian>(shard_range.end.into())
            },
        )?;
        Vector::write(
            &mut writer,
            &self.scan_targets.iter().collect::<Vec<_>>(),
            |w, &scan_target| scan_target.write(w),
        )
    }
}

impl TreeBounds {
    fn serialized_version() -> u8 {
        // Version 1 appends the ironwood tree sizes.
        1
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let version = read_version(&mut reader, "TreeBounds", Self::serialized_version())?;
        let sapling_initial_tree_size = reader.read_u32::<LittleEndian>()?;
        let sapling_final_tree_size = reader.read_u32::<LittleEndian>()?;
        let orchard_initial_tree_size = reader.read_u32::<LittleEndian>()?;
        let orchard_final_tree_size = reader.read_u32::<LittleEndian>()?;
        let (ironwood_initial_tree_size, ironwood_final_tree_size) = if version >= 1 {
            (
                reader.read_u32::<LittleEndian>()?,
                reader.read_u32::<LittleEndian>()?,
            )
        } else {
            (0, 0)
        };

        Ok(Self {
            sapling_initial_tree_size,
            sapling_final_tree_size,
            orchard_initial_tree_size,
            orchard_final_tree_size,
            ironwood_initial_tree_size,
            ironwood_final_tree_size,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        writer.write_u32::<LittleEndian>(self.sapling_initial_tree_size)?;
        writer.write_u32::<LittleEndian>(self.sapling_final_tree_size)?;
        writer.write_u32::<LittleEndian>(self.orchard_initial_tree_size)?;
        writer.write_u32::<LittleEndian>(self.orchard_final_tree_size)?;
        writer.write_u32::<LittleEndian>(self.ironwood_initial_tree_size)?;
        writer.write_u32::<LittleEndian>(self.ironwood_final_tree_size)
    }
}

impl NullifierMap {
    fn serialized_version() -> u8 {
        // Version 2 appends the ironwood nullifier map.
        2
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let version = read_version(&mut reader, "NullifierMap", Self::serialized_version())?;
        let sapling = Vector::read(&mut reader, |r| {
            let nullifier_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut *r)?;
            let nullifier =
                sapling_crypto::Nullifier::from_slice(&nullifier_bytes).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("failed to read nullifier. {e}"),
                    )
                })?;
            let scan_target = if version >= 1 {
                ScanTarget::read(r)?
            } else {
                let block_height = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
                let txid = TxId::read(r)?;

                ScanTarget {
                    block_height,
                    txid,
                    narrow_scan_area: false,
                }
            };

            Ok((nullifier, scan_target))
        })?
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let orchard = Vector::read(&mut reader, |r| {
            let nullifier = read_orchard_nullifier(&mut *r)?;
            let scan_target = if version >= 1 {
                ScanTarget::read(r)?
            } else {
                let block_height = BlockHeight::from_u32(r.read_u32::<LittleEndian>()?);
                let txid = TxId::read(r)?;

                ScanTarget {
                    block_height,
                    txid,
                    narrow_scan_area: false,
                }
            };

            Ok((nullifier, scan_target))
        })?
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let ironwood = if version >= 2 {
            Vector::read(&mut reader, |r| {
                let nullifier = read_orchard_nullifier(&mut *r)?;
                let scan_target = ScanTarget::read(r)?;

                Ok((nullifier, scan_target))
            })?
            .into_iter()
            .collect::<BTreeMap<_, _>>()
        } else {
            BTreeMap::new()
        };

        Ok(NullifierMap {
            sapling,
            orchard,
            ironwood,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        Vector::write(
            &mut writer,
            &self.sapling.iter().collect::<Vec<_>>(),
            |w, &(&nullifier, &scan_target)| {
                w.write_all(nullifier.as_ref())?;
                scan_target.write(w)
            },
        )?;
        Vector::write(
            &mut writer,
            &self.orchard.iter().collect::<Vec<_>>(),
            |w, &(&nullifier, &scan_target)| {
                w.write_all(&nullifier.to_bytes())?;
                scan_target.write(w)
            },
        )?;
        Vector::write(
            &mut writer,
            &self.ironwood.iter().collect::<Vec<_>>(),
            |w, &(&nullifier, &scan_target)| {
                w.write_all(&nullifier.to_bytes())?;
                scan_target.write(w)
            },
        )
    }
}

impl WalletBlock {
    fn serialized_version() -> u8 {
        0
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        read_version(&mut reader, "WalletBlock", Self::serialized_version())?;
        let block_height = BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?);
        let block_hash = BlockHash(read_array::<BLOCK_HASH_SIZE>(&mut reader)?);
        let prev_hash = BlockHash(read_array::<BLOCK_HASH_SIZE>(&mut reader)?);
        let time = reader.read_u32::<LittleEndian>()?;
        let txids = Vector::read(&mut reader, |r| TxId::read(r))?;
        let tree_bounds = TreeBounds::read(&mut reader)?;

        Ok(Self {
            block_height,
            block_hash,
            prev_hash,
            time,
            txids,
            tree_bounds,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        writer.write_u32::<LittleEndian>(self.block_height.into())?;
        writer.write_all(&self.block_hash.0)?;
        writer.write_all(&self.prev_hash.0)?;
        writer.write_u32::<LittleEndian>(self.time)?;
        Vector::write(&mut writer, self.txids(), |w, txid| txid.write(w))?;
        self.tree_bounds.write(&mut writer)
    }
}

impl WalletTransaction {
    fn serialized_version() -> u8 {
        // Version 1 appends the ironwood note collections.
        1
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(
        mut reader: R,
        consensus_parameters: &impl consensus::Parameters,
    ) -> std::io::Result<Self> {
        let version = read_version(&mut reader, "WalletTransaction", Self::serialized_version())?;
        let txid = TxId::read(&mut reader)?;
        let status = ConfirmationStatus::read(&mut reader)?;
        let transaction = Transaction::read(
            &mut reader,
            consensus::BranchId::for_height(consensus_parameters, status.get_height()),
        )?;
        let datetime = reader.read_u32::<LittleEndian>()?;
        let transparent_coins = Vector::read(&mut reader, |r| TransparentCoin::read(r))?;
        let sapling_notes = Vector::read(&mut reader, |r| SaplingNote::read(r))?;
        let orchard_notes = Vector::read(&mut reader, |r| OrchardNote::read(r))?;
        let outgoing_sapling_notes = Vector::read(&mut reader, |r| {
            OutgoingSaplingNote::read(r, consensus_parameters)
        })?;
        let outgoing_orchard_notes = Vector::read(&mut reader, |r| {
            OutgoingOrchardNote::read(r, consensus_parameters)
        })?;
        let (ironwood_notes, outgoing_ironwood_notes) = if version >= 1 {
            (
                Vector::read(&mut reader, |r| IronwoodNote::read(r))?,
                Vector::read(&mut reader, |r| {
                    OutgoingIronwoodNote::read(r, consensus_parameters)
                })?,
            )
        } else {
            (Vec::new(), Vec::new())
        };

        Ok(Self {
            txid,
            status,
            transaction,
            datetime,
            transparent_coins,
            sapling_notes,
            orchard_notes,
            ironwood_notes,
            outgoing_sapling_notes,
            outgoing_orchard_notes,
            outgoing_ironwood_notes,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(
        &self,
        mut writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        self.txid.write(&mut writer)?;
        self.status.write(&mut writer)?;
        self.transaction.write(&mut writer)?;
        writer.write_u32::<LittleEndian>(self.datetime)?;
        Vector::write(&mut writer, self.transparent_coins(), |w, output| {
            output.write(w)
        })?;
        Vector::write(&mut writer, self.sapling_notes(), |w, output| {
            output.write(w)
        })?;
        Vector::write(&mut writer, self.orchard_notes(), |w, output| {
            output.write(w)
        })?;
        Vector::write(&mut writer, self.outgoing_sapling_notes(), |w, output| {
            output.write(w, consensus_parameters)
        })?;
        Vector::write(&mut writer, self.outgoing_orchard_notes(), |w, output| {
            output.write(w, consensus_parameters)
        })?;
        Vector::write(&mut writer, self.ironwood_notes(), |w, output| {
            output.write(w)
        })?;
        Vector::write(&mut writer, self.outgoing_ironwood_notes(), |w, output| {
            output.write(w, consensus_parameters)
        })
    }
}

impl TransparentCoin {
    fn serialized_version() -> u8 {
        1
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let version = read_version(&mut reader, "TransparentCoin", Self::serialized_version())?;

        let txid = TxId::read(&mut reader)?;
        let output_index = if version >= 1 {
            reader.read_u32::<LittleEndian>()?
        } else {
            u32::from(reader.read_u16::<LittleEndian>()?)
        };

        let account_id =
            zip32::AccountId::try_from(reader.read_u32::<LittleEndian>()?).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to read account id. {e}"),
                )
            })?;
        let scope = TransparentScope::try_from(reader.read_u8()?)?;
        let address_index = reader.read_u32::<LittleEndian>()?;

        let address = read_string(&mut reader)?;
        let script = Script::read(&mut reader)?;
        let value = Zatoshis::from_u64(reader.read_u64::<LittleEndian>()?).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to read value. {e:?}"),
            )
        })?;
        let spending_transaction = Optional::read(&mut reader, TxId::read)?;

        Ok(Self {
            output_id: OutputId { txid, output_index },
            key_id: TransparentAddressId::new(
                account_id,
                scope,
                parse_field(
                    NonHardenedChildIndex::from_index(address_index),
                    "non-hardened child index",
                )?,
            ),
            address,
            value,
            script,
            spending_transaction,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;

        self.output_id.txid().write(&mut writer)?;
        writer.write_u32::<LittleEndian>(self.output_id.output_index())?;

        writer.write_u32::<LittleEndian>(self.key_id.account_id().into())?;
        writer.write_u8(self.key_id.scope() as u8)?;
        writer.write_u32::<LittleEndian>(self.key_id.address_index().index())?;

        write_string(&mut writer, &self.address)?;
        self.script.write(&mut writer)?;
        writer.write_u64::<LittleEndian>(self.value())?;
        Optional::write(&mut writer, self.spending_transaction, |w, txid| {
            txid.write(w)
        })?;

        Ok(())
    }
}

impl<N, Nf: Copy, P> WalletNote<N, Nf, P> {
    fn serialized_version() -> u8 {
        2
    }
}

fn read_refetch_nullifier_ranges(
    reader: &mut impl Read,
    version: u8,
) -> std::io::Result<Vec<Range<BlockHeight>>> {
    if version >= 1 {
        Vector::read(reader, |r| {
            let start = r.read_u32::<LittleEndian>()?;
            let end = r.read_u32::<LittleEndian>()?;
            Ok(BlockHeight::from_u32(start)..BlockHeight::from_u32(end))
        })
    } else {
        Ok(Vec::new())
    }
}

fn write_refetch_nullifier_ranges(
    writer: &mut impl Write,
    ranges: &[Range<BlockHeight>],
) -> std::io::Result<()> {
    Vector::write(writer, ranges, |w, range| {
        w.write_u32::<LittleEndian>(range.start.into())?;
        w.write_u32::<LittleEndian>(range.end.into())
    })
}

impl SaplingNote {
    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let version = read_version(&mut reader, "SaplingNote", Self::serialized_version())?;

        let txid = TxId::read(&mut reader)?;
        let output_index = if version >= 2 {
            reader.read_u32::<LittleEndian>()?
        } else {
            u32::from(reader.read_u16::<LittleEndian>()?)
        };

        let account_id =
            zip32::AccountId::try_from(reader.read_u32::<LittleEndian>()?).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to read account id. {e}"),
                )
            })?;
        let scope = match reader.read_u8()? {
            0 => Ok(zip32::Scope::External),
            1 => Ok(zip32::Scope::Internal),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid scope value",
            )),
        }?;

        let address_bytes = read_array::<RAW_ADDRESS_SIZE>(&mut reader)?;
        let recipient = parse_field(
            sapling_crypto::PaymentAddress::from_bytes(&address_bytes),
            "sapling payment address",
        )?;
        let value = sapling_crypto::value::NoteValue::from_raw(reader.read_u64::<LittleEndian>()?);
        let rseed_zip212 = reader.read_u8()?;
        let rseed_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut reader)?;
        let rseed = match rseed_zip212 {
            0 => sapling_crypto::Rseed::BeforeZip212(parse_field(
                jubjub::Fr::from_bytes(&rseed_bytes),
                "jubjub rseed",
            )?),
            1 => sapling_crypto::Rseed::AfterZip212(rseed_bytes),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid rseed zip212 byte",
                ));
            }
        };

        let nullifier = Optional::read(&mut reader, |r| {
            let nullifier_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut *r)?;

            sapling_crypto::Nullifier::from_slice(&nullifier_bytes).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to read nullifier. {e}"),
                )
            })
        })?;
        let position = Optional::read(&mut reader, |r| {
            Ok(Position::from(r.read_u64::<LittleEndian>()?))
        })?;
        let memo_bytes = read_array::<MEMO_SIZE>(&mut reader)?;
        let memo = Memo::from_bytes(&memo_bytes).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to read memo. {e}"),
            )
        })?;

        let spending_transaction = Optional::read(&mut reader, TxId::read)?;
        let refetch_nullifier_ranges = read_refetch_nullifier_ranges(&mut reader, version)?;

        Ok(Self {
            output_id: OutputId::new(txid, output_index),
            key_id: KeyId::from_parts(account_id, scope),
            note: sapling_crypto::Note::from_parts(recipient, value, rseed),
            nullifier,
            position,
            memo,
            spending_transaction,
            refetch_nullifier_ranges,
            marker: std::marker::PhantomData,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;

        self.output_id.txid().write(&mut writer)?;
        writer.write_u32::<LittleEndian>(self.output_id.output_index())?;

        writer.write_u32::<LittleEndian>(self.key_id.account_id.into())?;
        writer.write_u8(self.key_id.scope as u8)?;

        writer.write_all(&self.note.recipient().to_bytes())?;
        writer.write_u64::<LittleEndian>(self.value())?;
        match self.note.rseed() {
            sapling_crypto::Rseed::BeforeZip212(fr) => {
                writer.write_u8(0)?;
                writer.write_all(&fr.to_bytes())?;
            }
            sapling_crypto::Rseed::AfterZip212(bytes) => {
                writer.write_u8(1)?;
                writer.write_all(bytes)?;
            }
        }

        Optional::write(&mut writer, self.nullifier, |w, nullifier| {
            w.write_all(nullifier.as_ref())
        })?;
        Optional::write(&mut writer, self.position, |w, position| {
            w.write_u64::<LittleEndian>(position.into())
        })?;
        writer.write_all(self.memo.encode().as_array())?;

        Optional::write(&mut writer, self.spending_transaction, |w, txid| {
            txid.write(w)
        })?;

        write_refetch_nullifier_ranges(&mut writer, &self.refetch_nullifier_ranges)
    }
}

/// Shared reader for the Orchard-protocol note layout. Orchard and Ironwood
/// notes serialize identically, differing only in the note version fixed at
/// construction.
fn read_orchard_protocol_note<R: Read, P>(
    mut reader: R,
    note_version: orchard::note::NoteVersion,
) -> std::io::Result<WalletNote<orchard::Note, orchard::note::Nullifier, P>> {
    let version = read_version(
        &mut reader,
        "OrchardNote",
        WalletNote::<orchard::Note, orchard::note::Nullifier, P>::serialized_version(),
    )?;

    let txid = TxId::read(&mut reader)?;
    let output_index = if version >= 2 {
        reader.read_u32::<LittleEndian>()?
    } else {
        u32::from(reader.read_u16::<LittleEndian>()?)
    };

    let account_id =
        zip32::AccountId::try_from(reader.read_u32::<LittleEndian>()?).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to read account id. {e}"),
            )
        })?;
    let scope = match reader.read_u8()? {
        0 => Ok(zip32::Scope::External),
        1 => Ok(zip32::Scope::Internal),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid scope value",
        )),
    }?;

    let address_bytes = read_array::<RAW_ADDRESS_SIZE>(&mut reader)?;
    let recipient = parse_field(
        orchard::Address::from_raw_address_bytes(&address_bytes),
        "orchard address",
    )?;
    let value = orchard::value::NoteValue::from_raw(reader.read_u64::<LittleEndian>()?);
    let rho_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut reader)?;
    let rho = parse_field(orchard::note::Rho::from_bytes(&rho_bytes), "orchard rho")?;
    let rseed_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut reader)?;
    let rseed = parse_field(
        orchard::note::RandomSeed::from_bytes(rseed_bytes, &rho),
        "orchard random seed",
    )?;

    let nullifier = Optional::read(&mut reader, |r| read_orchard_nullifier(&mut *r))?;
    let position = Optional::read(&mut reader, |r| {
        Ok(Position::from(r.read_u64::<LittleEndian>()?))
    })?;
    let memo_bytes = read_array::<MEMO_SIZE>(&mut reader)?;
    let memo = Memo::from_bytes(&memo_bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to read memo. {e}"),
        )
    })?;

    let spending_transaction = Optional::read(&mut reader, TxId::read)?;
    let refetch_nullifier_ranges = read_refetch_nullifier_ranges(&mut reader, version)?;

    Ok(WalletNote {
        output_id: OutputId::new(txid, output_index),
        key_id: KeyId::from_parts(account_id, scope),
        note: parse_field(
            orchard::note::Note::from_parts(recipient, value, rho, rseed, note_version),
            "orchard note",
        )?,
        nullifier,
        position,
        memo,
        spending_transaction,
        refetch_nullifier_ranges,
        marker: std::marker::PhantomData,
    })
}

/// Shared writer for the Orchard-protocol note layout.
fn write_orchard_protocol_note<W: Write, P>(
    note: &WalletNote<orchard::Note, orchard::note::Nullifier, P>,
    mut writer: W,
) -> std::io::Result<()> {
    writer
        .write_u8(WalletNote::<orchard::Note, orchard::note::Nullifier, P>::serialized_version())?;

    note.output_id.txid().write(&mut writer)?;
    writer.write_u32::<LittleEndian>(note.output_id.output_index())?;

    writer.write_u32::<LittleEndian>(note.key_id.account_id.into())?;
    writer.write_u8(note.key_id.scope as u8)?;

    writer.write_all(&note.note.recipient().to_raw_address_bytes())?;
    writer.write_u64::<LittleEndian>(note.note.value().inner())?;
    writer.write_all(&note.note.rho().to_bytes())?;
    writer.write_all(note.note.rseed().as_bytes())?;

    Optional::write(&mut writer, note.nullifier, |w, nullifier| {
        w.write_all(&nullifier.to_bytes())
    })?;
    Optional::write(&mut writer, note.position, |w, position| {
        w.write_u64::<LittleEndian>(position.into())
    })?;
    writer.write_all(note.memo.encode().as_array())?;
    Optional::write(&mut writer, note.spending_transaction, |w, txid| {
        txid.write(w)
    })?;

    write_refetch_nullifier_ranges(&mut writer, &note.refetch_nullifier_ranges)
}

impl OrchardNote {
    /// Deserialize into `reader`
    pub fn read<R: Read>(reader: R) -> std::io::Result<Self> {
        read_orchard_protocol_note(reader, orchard::note::NoteVersion::V2)
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, writer: W) -> std::io::Result<()> {
        write_orchard_protocol_note(self, writer)
    }
}

impl IronwoodNote {
    /// Deserialize into `reader`
    pub fn read<R: Read>(reader: R) -> std::io::Result<Self> {
        read_orchard_protocol_note(reader, orchard::note::NoteVersion::V3)
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, writer: W) -> std::io::Result<()> {
        write_orchard_protocol_note(self, writer)
    }
}

impl<N, P> OutgoingNote<N, P> {
    fn serialized_version() -> u8 {
        1
    }
}

impl OutgoingSaplingNote {
    /// Deserialize into `reader`
    pub fn read<R: Read>(
        mut reader: R,
        consensus_parameters: &impl consensus::Parameters,
    ) -> std::io::Result<Self> {
        let version = read_version(
            &mut reader,
            "OutgoingSaplingNote",
            Self::serialized_version(),
        )?;

        let txid = TxId::read(&mut reader)?;
        let output_index = if version >= 1 {
            reader.read_u32::<LittleEndian>()?
        } else {
            u32::from(reader.read_u16::<LittleEndian>()?)
        };

        let account_id =
            zip32::AccountId::try_from(reader.read_u32::<LittleEndian>()?).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("failed to read account id. {e}"),
                )
            })?;
        let scope = match reader.read_u8()? {
            0 => Ok(zip32::Scope::External),
            1 => Ok(zip32::Scope::Internal),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid scope value",
            )),
        }?;

        let address_bytes = read_array::<RAW_ADDRESS_SIZE>(&mut reader)?;
        let recipient = parse_field(
            sapling_crypto::PaymentAddress::from_bytes(&address_bytes),
            "sapling payment address",
        )?;
        let value = sapling_crypto::value::NoteValue::from_raw(reader.read_u64::<LittleEndian>()?);
        let rseed_zip212 = reader.read_u8()?;
        let rseed_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut reader)?;
        let rseed = match rseed_zip212 {
            0 => sapling_crypto::Rseed::BeforeZip212(parse_field(
                jubjub::Fr::from_bytes(&rseed_bytes),
                "jubjub rseed",
            )?),
            1 => sapling_crypto::Rseed::AfterZip212(rseed_bytes),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid rseed zip212 byte",
                ));
            }
        };

        let memo_bytes = read_array::<MEMO_SIZE>(&mut reader)?;
        let memo = Memo::from_bytes(&memo_bytes).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to read memo. {e}"),
            )
        })?;

        let recipient_unified_address = Optional::read(&mut reader, |r| {
            let encoded_address = read_string(r)?;

            decode_unified_address(consensus_parameters, &encoded_address)
        })?;

        Ok(Self {
            output_id: OutputId::new(txid, output_index),
            key_id: KeyId::from_parts(account_id, scope),
            note: sapling_crypto::Note::from_parts(recipient, value, rseed),
            memo,
            recipient_full_unified_address: recipient_unified_address,
            marker: std::marker::PhantomData,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(
        &self,
        mut writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;

        self.output_id.txid().write(&mut writer)?;
        writer.write_u32::<LittleEndian>(self.output_id.output_index())?;

        writer.write_u32::<LittleEndian>(self.key_id.account_id.into())?;
        writer.write_u8(self.key_id.scope as u8)?;

        writer.write_all(&self.note.recipient().to_bytes())?;
        writer.write_u64::<LittleEndian>(self.value())?;
        match self.note.rseed() {
            sapling_crypto::Rseed::BeforeZip212(fr) => {
                writer.write_u8(0)?;
                writer.write_all(&fr.to_bytes())?;
            }
            sapling_crypto::Rseed::AfterZip212(bytes) => {
                writer.write_u8(1)?;
                writer.write_all(bytes)?;
            }
        }

        writer.write_all(self.memo.encode().as_array())?;
        Optional::write(
            &mut writer,
            self.recipient_full_unified_address.as_ref(),
            |w, unified_address| write_string(w, &unified_address.encode(consensus_parameters)),
        )?;

        Ok(())
    }
}

/// Shared reader for the Orchard-protocol outgoing note layout. Orchard and
/// Ironwood outgoing notes serialize identically, differing only in the note
/// version fixed at construction.
fn read_orchard_protocol_outgoing_note<R: Read, P>(
    mut reader: R,
    consensus_parameters: &impl consensus::Parameters,
    note_version: orchard::note::NoteVersion,
) -> std::io::Result<OutgoingNote<orchard::Note, P>> {
    let version = read_version(
        &mut reader,
        "OutgoingOrchardNote",
        OutgoingNote::<orchard::Note, P>::serialized_version(),
    )?;

    let txid = TxId::read(&mut reader)?;
    let output_index = if version >= 1 {
        reader.read_u32::<LittleEndian>()?
    } else {
        u32::from(reader.read_u16::<LittleEndian>()?)
    };

    let account_id =
        zip32::AccountId::try_from(reader.read_u32::<LittleEndian>()?).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to read account id. {e}"),
            )
        })?;
    let scope = match reader.read_u8()? {
        0 => Ok(zip32::Scope::External),
        1 => Ok(zip32::Scope::Internal),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid scope value",
        )),
    }?;

    let address_bytes = read_array::<RAW_ADDRESS_SIZE>(&mut reader)?;
    let recipient = parse_field(
        orchard::Address::from_raw_address_bytes(&address_bytes),
        "orchard address",
    )?;
    let value = orchard::value::NoteValue::from_raw(reader.read_u64::<LittleEndian>()?);
    let rho_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut reader)?;
    let rho = parse_field(orchard::note::Rho::from_bytes(&rho_bytes), "orchard rho")?;
    let rseed_bytes = read_array::<FIELD_ELEMENT_SIZE>(&mut reader)?;
    let rseed = parse_field(
        orchard::note::RandomSeed::from_bytes(rseed_bytes, &rho),
        "orchard random seed",
    )?;

    let memo_bytes = read_array::<MEMO_SIZE>(&mut reader)?;
    let memo = Memo::from_bytes(&memo_bytes).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to read memo. {e}"),
        )
    })?;

    let recipient_unified_address = Optional::read(&mut reader, |r| {
        let encoded_address = read_string(r)?;

        decode_unified_address(consensus_parameters, &encoded_address)
    })?;

    Ok(OutgoingNote {
        output_id: OutputId::new(txid, output_index),
        key_id: KeyId::from_parts(account_id, scope),
        note: parse_field(
            orchard::note::Note::from_parts(recipient, value, rho, rseed, note_version),
            "orchard note",
        )?,
        memo,
        recipient_full_unified_address: recipient_unified_address,
        marker: std::marker::PhantomData,
    })
}

/// Shared writer for the Orchard-protocol outgoing note layout.
fn write_orchard_protocol_outgoing_note<W: Write, P>(
    note: &OutgoingNote<orchard::Note, P>,
    mut writer: W,
    consensus_parameters: &impl consensus::Parameters,
) -> std::io::Result<()> {
    writer.write_u8(OutgoingNote::<orchard::Note, P>::serialized_version())?;

    note.output_id.txid().write(&mut writer)?;
    writer.write_u32::<LittleEndian>(note.output_id.output_index())?;

    writer.write_u32::<LittleEndian>(note.key_id.account_id.into())?;
    writer.write_u8(note.key_id.scope as u8)?;

    writer.write_all(&note.note.recipient().to_raw_address_bytes())?;
    writer.write_u64::<LittleEndian>(note.note.value().inner())?;
    writer.write_all(&note.note.rho().to_bytes())?;
    writer.write_all(note.note.rseed().as_bytes())?;

    writer.write_all(note.memo.encode().as_array())?;
    Optional::write(
        &mut writer,
        note.recipient_full_unified_address.as_ref(),
        |w, unified_address| write_string(w, &unified_address.encode(consensus_parameters)),
    )?;

    Ok(())
}

impl OutgoingOrchardNote {
    /// Deserialize into `reader`
    pub fn read<R: Read>(
        reader: R,
        consensus_parameters: &impl consensus::Parameters,
    ) -> std::io::Result<Self> {
        read_orchard_protocol_outgoing_note(
            reader,
            consensus_parameters,
            orchard::note::NoteVersion::V2,
        )
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(
        &self,
        writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> std::io::Result<()> {
        write_orchard_protocol_outgoing_note(self, writer, consensus_parameters)
    }
}

impl OutgoingIronwoodNote {
    /// Deserialize into `reader`
    pub fn read<R: Read>(
        reader: R,
        consensus_parameters: &impl consensus::Parameters,
    ) -> std::io::Result<Self> {
        read_orchard_protocol_outgoing_note(
            reader,
            consensus_parameters,
            orchard::note::NoteVersion::V3,
        )
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(
        &self,
        writer: W,
        consensus_parameters: &impl consensus::Parameters,
    ) -> std::io::Result<()> {
        write_orchard_protocol_outgoing_note(self, writer, consensus_parameters)
    }
}

impl ShardTrees {
    fn serialized_version() -> u8 {
        // Version 1 appends the Ironwood shard tree after the Orchard one.
        1
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let version = read_version(&mut reader, "ShardTrees", Self::serialized_version())?;
        let sapling = Self::read_shardtree(&mut reader)?;
        let orchard = Self::read_shardtree(&mut reader)?;
        let ironwood = if version >= 1 {
            Self::read_shardtree(&mut reader)?
        } else {
            // Pre-Ironwood wallet files: start with an empty Ironwood tree.
            Self::new().ironwood
        };

        Ok(Self {
            sapling,
            orchard,
            ironwood,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&mut self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        Self::write_shardtree(&mut writer, &mut self.sapling)?;
        Self::write_shardtree(&mut writer, &mut self.orchard)?;
        Self::write_shardtree(&mut writer, &mut self.ironwood)?;

        Ok(())
    }

    fn read_shardtree<
        H: Hashable + Clone + HashSer + Eq,
        C: Ord + std::fmt::Debug + Copy + From<u32>,
        R: Read,
        const DEPTH: u8,
        const SHARD_HEIGHT: u8,
    >(
        mut reader: R,
    ) -> std::io::Result<ShardTree<MemoryShardStore<H, C>, DEPTH, SHARD_HEIGHT>> {
        let shards = Vector::read(&mut reader, |r| {
            let level = incrementalmerkletree::Level::from(r.read_u8()?);
            let index = r.read_u64::<LittleEndian>()?;
            let root_addr = incrementalmerkletree::Address::from_parts(level, index);
            let shard = read_shard(r)?;

            LocatedPrunableTree::from_parts(root_addr, shard).map_err(|addr| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("parent node in root has level 0 relative to root address: {addr:?}"),
                )
            })
        })?;
        let mut store = MemoryShardStore::empty();
        for shard in shards {
            store.put_shard(shard).expect("infallible");
        }
        let checkpoints = Vector::read(&mut reader, |r| {
            let checkpoint_id = C::from(r.read_u32::<LittleEndian>()?);
            let tree_state = match r.read_u8()? {
                0 => TreeState::Empty,
                1 => TreeState::AtPosition(Position::from(r.read_u64::<LittleEndian>()?)),
                otherwise => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "failed to read TreeState. expected boolean value, found {otherwise}"
                        ),
                    ));
                }
            };
            let marks_removed =
                Vector::read(r, |r| r.read_u64::<LittleEndian>().map(Position::from))?;
            Ok((
                checkpoint_id,
                Checkpoint::from_parts(tree_state, marks_removed.into_iter().collect()),
            ))
        })?;
        for (checkpoint_id, checkpoint) in checkpoints {
            store
                .add_checkpoint(checkpoint_id, checkpoint)
                .expect("Infallible");
        }
        store.put_cap(read_shard(reader)?).expect("Infallible");

        Ok(shardtree::ShardTree::new(
            store,
            MAX_REORG_ALLOWANCE as usize,
        ))
    }

    /// Write memory-backed shardstore, represented tree.
    fn write_shardtree<
        H: Hashable + Clone + Eq + HashSer,
        C: Ord + std::fmt::Debug + Copy,
        W: Write,
        const DEPTH: u8,
        const SHARD_HEIGHT: u8,
    >(
        mut writer: W,
        shardtree: &mut ShardTree<MemoryShardStore<H, C>, DEPTH, SHARD_HEIGHT>,
    ) -> std::io::Result<()>
    where
        u32: From<C>,
    {
        fn write_shards<W, H, C>(
            mut writer: W,
            store: &MemoryShardStore<H, C>,
        ) -> std::io::Result<()>
        where
            H: Hashable + Clone + Eq + HashSer,
            C: Ord + std::fmt::Debug + Copy,
            W: Write,
        {
            let roots = store.get_shard_roots().expect("Infallible");
            Vector::write(&mut writer, &roots, |w, root| {
                w.write_u8(root.level().into())?;
                w.write_u64::<LittleEndian>(root.index())?;
                let shard = store
                    .get_shard(*root)
                    .expect("Infallible")
                    .expect("cannot find root that shard store claims to have");
                write_shard(w, shard.root())
            })
        }

        fn write_checkpoints<W, Cid>(
            mut writer: W,
            checkpoints: &[(Cid, Checkpoint)],
        ) -> std::io::Result<()>
        where
            W: Write,
            Cid: Ord + std::fmt::Debug + Copy,
            u32: From<Cid>,
        {
            Vector::write(
                &mut writer,
                checkpoints,
                |mut w, (checkpoint_id, checkpoint)| {
                    w.write_u32::<LittleEndian>(u32::from(*checkpoint_id))?;
                    match checkpoint.tree_state() {
                        shardtree::store::TreeState::Empty => w.write_u8(0),
                        shardtree::store::TreeState::AtPosition(pos) => {
                            w.write_u8(1)?;
                            w.write_u64::<LittleEndian>(<u64 as From<Position>>::from(pos))
                        }
                    }?;
                    Vector::write(
                        &mut w,
                        &checkpoint.marks_removed().iter().collect::<Vec<_>>(),
                        |w, mark| {
                            w.write_u64::<LittleEndian>(<u64 as From<Position>>::from(**mark))
                        },
                    )
                },
            )
        }

        // Replace original tree with empty tree, and mutate new version into store.
        let mut store = std::mem::replace(
            shardtree,
            shardtree::ShardTree::new(MemoryShardStore::empty(), 0),
        )
        .into_store();

        macro_rules! write_with_error_handling {
            ($writer: ident, $from: ident) => {
                if let Err(e) = $writer(&mut writer, &$from) {
                    *shardtree = shardtree::ShardTree::new(store, MAX_REORG_ALLOWANCE as usize);
                    return Err(e);
                }
            };
        }

        // Write located prunable trees
        write_with_error_handling!(write_shards, store);

        // Write checkpoints
        let mut checkpoints = Vec::new();
        let checkpoint_count = store.checkpoint_count().expect("Infallible");
        store
            .with_checkpoints(checkpoint_count, |checkpoint_id, checkpoint| {
                checkpoints.push((*checkpoint_id, checkpoint.clone()));
                Ok(())
            })
            .expect("Infallible");
        if checkpoints.len() > MAX_REORG_ALLOWANCE as usize {
            let keep_from = checkpoints.len() - MAX_REORG_ALLOWANCE as usize;
            checkpoints.drain(..keep_from);
        }
        write_with_error_handling!(write_checkpoints, checkpoints);

        // Write cap
        let cap = store.get_cap().expect("Infallible");
        write_with_error_handling!(write_shard, cap);

        *shardtree = shardtree::ShardTree::new(store, MAX_REORG_ALLOWANCE as usize);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid_data(e: &std::io::Error) -> bool {
        e.kind() == std::io::ErrorKind::InvalidData
    }

    // REPRO (a): `NullifierMap::read` uses `.expect` on
    // `orchard::note::Nullifier::from_bytes`, so a corrupt nullifier panics
    // instead of returning `io::Error(InvalidData)`.
    #[test]
    fn nullifier_map_read_rejects_non_canonical_orchard_nullifier() {
        let mut map = NullifierMap::new();
        let nullifier = orchard::note::Nullifier::from_bytes(&[0u8; 32]).unwrap();
        map.orchard.insert(
            nullifier,
            ScanTarget {
                block_height: BlockHeight::from_u32(10),
                txid: TxId::from_bytes([1u8; 32]),
                narrow_scan_area: false,
            },
        );
        let mut bytes = Vec::new();
        map.write(&mut bytes).expect("write should succeed");

        // Layout: version(1) | sapling vec len(1, = 0) | orchard vec len(1, = 1) | 32 nullifier bytes | ...
        let nullifier_offset = 3;
        assert_eq!(&bytes[nullifier_offset..nullifier_offset + 32], &[0u8; 32]);
        // All 0xFF is not a canonical Pallas base field element.
        bytes[nullifier_offset..nullifier_offset + 32].copy_from_slice(&[0xFFu8; 32]);
        assert!(bool::from(
            orchard::note::Nullifier::from_bytes(&[0xFFu8; 32]).is_none()
        ));

        let result = std::panic::catch_unwind(|| NullifierMap::read(bytes.as_slice()));
        match result {
            Ok(Err(e)) => assert!(invalid_data(&e), "unexpected error kind: {e}"),
            Ok(Ok(_)) => panic!("corrupt orchard nullifier was accepted"),
            Err(_) => panic!("NullifierMap::read panicked on a corrupt orchard nullifier"),
        }
    }

    // REPRO (b): `read_string` allocates `vec![0; len]` from an untrusted u64
    // length before reading any bytes, so a corrupt length aborts or panics
    // instead of returning an error.
    #[test]
    fn read_string_rejects_oversized_length_without_allocating() {
        let mut bytes = Vec::new();
        bytes.write_u64::<LittleEndian>(usize::MAX as u64).unwrap();

        let result = std::panic::catch_unwind(|| read_string(bytes.as_slice()));
        match result {
            Ok(Err(_)) => {}
            Ok(Ok(s)) => panic!("read_string returned a string of length {}", s.len()),
            Err(_) => panic!("read_string panicked on an oversized length prefix"),
        }
    }

    // REPRO (d): the `read_string` truncation guard returned a bare
    // `UnexpectedEof` with no message, hiding both the claimed and the
    // delivered length and contradicting the `InvalidData` contract that
    // issue #2732 states.
    #[test]
    fn read_string_truncation_error_names_both_lengths() {
        const CLAIMED_LENGTH: u64 = 41;
        const DELIVERED_BYTES: &[u8] = b"zingo!!";

        let mut bytes = Vec::new();
        bytes.write_u64::<LittleEndian>(CLAIMED_LENGTH).unwrap();
        bytes.extend_from_slice(DELIVERED_BYTES);

        let error = match read_string(bytes.as_slice()) {
            Err(e) => e,
            Ok(s) => panic!("truncated stream was accepted as string {s:?}"),
        };
        assert!(
            invalid_data(&error),
            "truncation error kind is {:?}, not InvalidData",
            error.kind()
        );
        let message = error.to_string();
        assert!(
            message.contains(&CLAIMED_LENGTH.to_string()),
            "error message {message:?} does not name the claimed length {CLAIMED_LENGTH}"
        );
        assert!(
            message.contains(&DELIVERED_BYTES.len().to_string()),
            "error message {message:?} does not name the delivered length {}",
            DELIVERED_BYTES.len()
        );
    }

    // REPRO (c): `SyncState::read` matches `3..` and never rejects a version
    // above `serialized_version()`.
    #[test]
    fn sync_state_read_rejects_unknown_future_version() {
        let mut state = SyncState::new();
        let mut bytes = Vec::new();
        state.write(&mut bytes).expect("write should succeed");
        assert_eq!(bytes[0], SyncState::serialized_version());
        bytes[0] = 99;

        let result = SyncState::read(bytes.as_slice());
        assert!(
            result.is_err(),
            "SyncState::read accepted unknown version 99 as the newest known layout"
        );
    }

    // REPRO (c): `SyncConfig::read` never rejects a version above
    // `serialized_version()`.
    #[test]
    fn sync_config_read_rejects_unknown_future_version() {
        let mut config = crate::config::SyncConfig::default();
        let mut bytes = Vec::new();
        config.write(&mut bytes).expect("write should succeed");
        bytes[0] = 99;

        let result = crate::config::SyncConfig::read(bytes.as_slice());
        assert!(
            result.is_err(),
            "SyncConfig::read accepted unknown version 99 as the newest known layout"
        );
    }

    // REPRO (c): `ScanTarget::read` and `WalletBlock::read` discard the
    // version byte entirely.
    #[test]
    fn scan_target_read_rejects_unknown_future_version() {
        let target = ScanTarget {
            block_height: BlockHeight::from_u32(10),
            txid: TxId::from_bytes([1u8; 32]),
            narrow_scan_area: false,
        };
        let mut bytes = Vec::new();
        target.write(&mut bytes).expect("write should succeed");
        bytes[0] = 99;

        assert!(
            ScanTarget::read(bytes.as_slice()).is_err(),
            "ScanTarget::read accepted unknown version 99"
        );
    }

    // GUARD (d): `SyncState::write` uses `priority as u8` (declaration order)
    // while the reader uses an explicit table. This passes today and pins the
    // current order so that a reorder of `ScanPriority` becomes visible.
    #[test]
    fn sync_state_roundtrip_preserves_every_scan_priority() {
        let priorities = [
            ScanPriority::RefetchingNullifiers,
            ScanPriority::Scanning,
            ScanPriority::Scanned,
            ScanPriority::ScannedWithoutMapping,
            ScanPriority::Historic,
            ScanPriority::OpenAdjacent,
            ScanPriority::FoundNote,
            ScanPriority::ChainTip,
            ScanPriority::Verify,
        ];
        let mut state = SyncState::new();
        for (i, priority) in priorities.iter().enumerate() {
            let start = (i as u32) * 10;
            state.scan_ranges.push(ScanRange::from_parts(
                BlockHeight::from_u32(start)..BlockHeight::from_u32(start + 10),
                *priority,
            ));
        }
        let mut bytes = Vec::new();
        state.write(&mut bytes).expect("write should succeed");
        let recovered = SyncState::read(bytes.as_slice()).expect("read should succeed");
        assert_eq!(recovered.scan_ranges, state.scan_ranges);
    }

    // Helper: build a minimal v3 SyncState byte blob (no ironwood_shard_ranges).
    // Format: version(1) | scan_ranges[0] | sapling_shard_ranges[0] |
    //         orchard_shard_ranges[0] | scan_targets[0]
    fn v3_sync_state_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        out.write_u8(3).unwrap();
        Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).unwrap();
        Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).unwrap();
        Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).unwrap();
        Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).unwrap();
        out
    }

    #[test]
    fn sync_state_v3_reads_with_empty_ironwood_ranges() {
        let bytes = v3_sync_state_bytes();
        let sync_state = SyncState::read(bytes.as_slice()).expect("v3 should read cleanly");
        assert!(sync_state.ironwood_shard_ranges().is_empty());
    }

    #[test]
    fn sync_state_v4_roundtrip_preserves_ironwood_ranges() {
        let mut state = SyncState::new();
        state.ironwood_shard_ranges = vec![
            BlockHeight::from_u32(100)..BlockHeight::from_u32(200),
            BlockHeight::from_u32(300)..BlockHeight::from_u32(400),
        ];
        state.scan_ranges.push(ScanRange::from_parts(
            BlockHeight::from_u32(100)..BlockHeight::from_u32(400),
            ScanPriority::Historic,
        ));
        let mut bytes = Vec::new();
        state.write(&mut bytes).expect("write should succeed");
        let recovered = SyncState::read(bytes.as_slice()).expect("read should succeed");
        assert_eq!(recovered.ironwood_shard_ranges, state.ironwood_shard_ranges);
        assert_eq!(recovered.scan_ranges, state.scan_ranges);
    }

    // Helper: build a minimal v1 NullifierMap byte blob (no ironwood BTreeMap).
    fn v1_nullifier_map_bytes() -> Vec<u8> {
        let mut out = Vec::new();
        out.write_u8(1).unwrap();
        Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).unwrap();
        Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).unwrap();
        out
    }

    #[test]
    fn nullifier_map_v1_reads_with_empty_ironwood_map() {
        let bytes = v1_nullifier_map_bytes();
        let map = NullifierMap::read(bytes.as_slice()).expect("v1 should read cleanly");
        assert!(map.ironwood.is_empty());
    }

    #[test]
    fn nullifier_map_v2_roundtrip_preserves_ironwood() {
        let map = NullifierMap::new();
        let mut bytes = Vec::new();
        map.write(&mut bytes).expect("write should succeed");
        let recovered = NullifierMap::read(bytes.as_slice()).expect("read should succeed");
        assert!(recovered.ironwood.is_empty());
    }

    // Helper: build a v0 TreeBounds byte blob (no ironwood tree sizes).
    fn v0_tree_bounds_bytes(
        sapling_initial: u32,
        sapling_final: u32,
        orchard_initial: u32,
        orchard_final: u32,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.write_u8(0).unwrap();
        out.write_u32::<LittleEndian>(sapling_initial).unwrap();
        out.write_u32::<LittleEndian>(sapling_final).unwrap();
        out.write_u32::<LittleEndian>(orchard_initial).unwrap();
        out.write_u32::<LittleEndian>(orchard_final).unwrap();
        out
    }

    #[test]
    fn tree_bounds_v0_reads_with_zero_ironwood_sizes() {
        let bytes = v0_tree_bounds_bytes(10, 20, 30, 40);
        let bounds = TreeBounds::read(bytes.as_slice()).expect("v0 should read cleanly");
        assert_eq!(bounds.sapling_initial_tree_size, 10);
        assert_eq!(bounds.orchard_final_tree_size, 40);
        assert_eq!(bounds.ironwood_initial_tree_size, 0);
        assert_eq!(bounds.ironwood_final_tree_size, 0);
    }

    #[test]
    fn tree_bounds_v1_roundtrip() {
        let bounds = TreeBounds {
            sapling_initial_tree_size: 1,
            sapling_final_tree_size: 2,
            orchard_initial_tree_size: 3,
            orchard_final_tree_size: 4,
            ironwood_initial_tree_size: 5,
            ironwood_final_tree_size: 6,
        };
        let mut bytes = Vec::new();
        bounds.write(&mut bytes).expect("write should succeed");
        let recovered = TreeBounds::read(bytes.as_slice()).expect("read should succeed");
        assert_eq!(recovered.ironwood_initial_tree_size, 5);
        assert_eq!(recovered.ironwood_final_tree_size, 6);
    }

    #[test]
    fn shardtree_roundtrip_keeps_newest_checkpoints() {
        let mut shard_trees = ShardTrees::new();

        for height in 1..=150 {
            let height = BlockHeight::from_u32(height);
            shard_trees
                .sapling
                .store_mut()
                .add_checkpoint(
                    height,
                    Checkpoint::from_parts(TreeState::Empty, BTreeSet::new()),
                )
                .expect("infallible");
            shard_trees
                .orchard
                .store_mut()
                .add_checkpoint(
                    height,
                    Checkpoint::from_parts(TreeState::Empty, BTreeSet::new()),
                )
                .expect("infallible");
        }

        let mut bytes = Vec::new();
        shard_trees.write(&mut bytes).expect("write should succeed");
        let roundtripped = ShardTrees::read(bytes.as_slice()).expect("read should succeed");

        let sapling_store = roundtripped.sapling.store();
        let orchard_store = roundtripped.orchard.store();

        assert_eq!(sapling_store.checkpoint_count().expect("infallible"), 100);
        assert_eq!(orchard_store.checkpoint_count().expect("infallible"), 100);
        assert_eq!(
            sapling_store.min_checkpoint_id().expect("infallible"),
            Some(BlockHeight::from_u32(51))
        );
        assert_eq!(
            sapling_store.max_checkpoint_id().expect("infallible"),
            Some(BlockHeight::from_u32(150))
        );
        assert_eq!(
            orchard_store.min_checkpoint_id().expect("infallible"),
            Some(BlockHeight::from_u32(51))
        );
        assert_eq!(
            orchard_store.max_checkpoint_id().expect("infallible"),
            Some(BlockHeight::from_u32(150))
        );
        assert!(
            sapling_store
                .get_checkpoint(&BlockHeight::from_u32(149))
                .expect("infallible")
                .is_some()
        );
        assert!(
            sapling_store
                .get_checkpoint(&BlockHeight::from_u32(50))
                .expect("infallible")
                .is_none()
        );
    }
}

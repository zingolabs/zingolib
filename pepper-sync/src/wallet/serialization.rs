//! Serialization and de-serialization of wallet structs in [`crate::wallet`] including utilities.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
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
    sync::{MAX_SHARDTREE_CHECKPOINTS, ScanPriority, ScanRange},
    wallet::ScanTarget,
};

use super::{
    InitialSyncState, IronwoodNote, KeyIdInterface, NullifierMap, OrchardNote,
    OutgoingIronwoodNote, OutgoingNote, OutgoingNoteInterface, OutgoingOrchardNote,
    OutgoingSaplingNote, OutputId, OutputInterface, SaplingNote, ShardTrees, SyncState,
    TransparentCoin, TreeBounds, WalletBlock, WalletNote, WalletTransaction,
};

fn read_string<R: Read>(mut reader: R) -> std::io::Result<String> {
    let str_len = reader.read_u64::<LittleEndian>()?;
    let mut str_bytes = vec![0; str_len as usize];
    reader.read_exact(&mut str_bytes)?;

    String::from_utf8(str_bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
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
        let _version = reader.read_u8()?;
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
        let version = reader.read_u8()?;
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
        let version = reader.read_u8()?;
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
        let version = reader.read_u8()?;
        let sapling = Vector::read(&mut reader, |r| {
            let mut nullifier_bytes = [0u8; 32];
            r.read_exact(&mut nullifier_bytes)?;
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
            let mut nullifier_bytes = [0u8; 32];
            r.read_exact(&mut nullifier_bytes)?;
            let nullifier = orchard::note::Nullifier::from_bytes(&nullifier_bytes)
                .expect("nullifier bytes should be valid");
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
                let mut nullifier_bytes = [0u8; 32];
                r.read_exact(&mut nullifier_bytes)?;
                let nullifier = orchard::note::Nullifier::from_bytes(&nullifier_bytes)
                    .expect("nullifier bytes should be valid");
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
        let _version = reader.read_u8()?;
        let block_height = BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?);
        let mut block_hash = BlockHash([0u8; 32]);
        reader.read_exact(&mut block_hash.0)?;
        let mut prev_hash = BlockHash([0u8; 32]);
        reader.read_exact(&mut prev_hash.0)?;
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
        let version = reader.read_u8()?;
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
        let version = reader.read_u8()?;

        let txid = TxId::read(&mut reader)?;
        let output_index = if version >= 1 {
            reader.read_u32::<LittleEndian>()?
        } else {
            u32::from(reader.read_u16::<LittleEndian>()?)
        };

        let account_id = zip32::AccountId::try_from(reader.read_u32::<LittleEndian>()?)
            .expect("only valid account ids written");
        let scope = TransparentScope::try_from(reader.read_u8()?)?;
        let address_index = reader.read_u32::<LittleEndian>()?;

        let address = read_string(&mut reader)?;
        let script = Script::read(&mut reader)?;
        let value = Zatoshis::from_u64(reader.read_u64::<LittleEndian>()?)
            .expect("only valid values written");
        let spending_transaction = Optional::read(&mut reader, TxId::read)?;

        Ok(Self {
            output_id: OutputId { txid, output_index },
            key_id: TransparentAddressId::new(
                account_id,
                scope,
                NonHardenedChildIndex::from_index(address_index)
                    .expect("only non-hardened child indexes should be written"),
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
        let version = reader.read_u8()?;

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

        let mut address_bytes = [0u8; 43];
        reader.read_exact(&mut address_bytes)?;
        let recipient =
            sapling_crypto::PaymentAddress::from_bytes(&address_bytes).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "failed to read payment address",
                )
            })?;
        let value = sapling_crypto::value::NoteValue::from_raw(reader.read_u64::<LittleEndian>()?);
        let rseed_zip212 = reader.read_u8()?;
        let mut rseed_bytes = [0u8; 32];
        reader.read_exact(&mut rseed_bytes)?;
        let rseed = match rseed_zip212 {
            0 => sapling_crypto::Rseed::BeforeZip212(
                jubjub::Fr::from_bytes(&rseed_bytes).expect("should read valid jubjub bytes"),
            ),
            1 => sapling_crypto::Rseed::AfterZip212(rseed_bytes),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid rseed zip212 byte",
                ));
            }
        };

        let nullifier = Optional::read(&mut reader, |r| {
            let mut nullifier_bytes = [0u8; 32];
            r.read_exact(&mut nullifier_bytes)?;

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
        let mut memo_bytes = [0u8; 512];
        reader.read_exact(&mut memo_bytes)?;
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
    let version = reader.read_u8()?;

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

    let mut address_bytes = [0u8; 43];
    reader.read_exact(&mut address_bytes)?;
    let recipient = orchard::Address::from_raw_address_bytes(&address_bytes)
        .expect("should be a valid address");
    let value = orchard::value::NoteValue::from_raw(reader.read_u64::<LittleEndian>()?);
    let mut rho_bytes = [0u8; 32];
    reader.read_exact(&mut rho_bytes)?;
    let rho = orchard::note::Rho::from_bytes(&rho_bytes).expect("should be valid rho bytes");
    let mut rseed_bytes = [0u8; 32];
    reader.read_exact(&mut rseed_bytes)?;
    let rseed = orchard::note::RandomSeed::from_bytes(rseed_bytes, &rho)
        .expect("should be valid random seed bytes");

    let nullifier = Optional::read(&mut reader, |r| {
        let mut nullifier_bytes = [0u8; 32];
        r.read_exact(&mut nullifier_bytes)?;

        Ok(orchard::note::Nullifier::from_bytes(&nullifier_bytes)
            .expect("should be valid nullfiier bytes"))
    })?;
    let position = Optional::read(&mut reader, |r| {
        Ok(Position::from(r.read_u64::<LittleEndian>()?))
    })?;
    let mut memo_bytes = [0u8; 512];
    reader.read_exact(&mut memo_bytes)?;
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
        note: orchard::note::Note::from_parts(recipient, value, rho, rseed, note_version)
            .expect("should be a valid orchard note"),
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
        let version = reader.read_u8()?;

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

        let mut address_bytes = [0u8; 43];
        reader.read_exact(&mut address_bytes)?;
        let recipient =
            sapling_crypto::PaymentAddress::from_bytes(&address_bytes).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "failed to read payment address",
                )
            })?;
        let value = sapling_crypto::value::NoteValue::from_raw(reader.read_u64::<LittleEndian>()?);
        let rseed_zip212 = reader.read_u8()?;
        let mut rseed_bytes = [0u8; 32];
        reader.read_exact(&mut rseed_bytes)?;
        let rseed = match rseed_zip212 {
            0 => sapling_crypto::Rseed::BeforeZip212(
                jubjub::Fr::from_bytes(&rseed_bytes).expect("should read valid jubjub bytes"),
            ),
            1 => sapling_crypto::Rseed::AfterZip212(rseed_bytes),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid rseed zip212 byte",
                ));
            }
        };

        let mut memo_bytes = [0u8; 512];
        reader.read_exact(&mut memo_bytes)?;
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
    let version = reader.read_u8()?;

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

    let mut address_bytes = [0u8; 43];
    reader.read_exact(&mut address_bytes)?;
    let recipient = orchard::Address::from_raw_address_bytes(&address_bytes)
        .expect("should be a valid address");
    let value = orchard::value::NoteValue::from_raw(reader.read_u64::<LittleEndian>()?);
    let mut rho_bytes = [0u8; 32];
    reader.read_exact(&mut rho_bytes)?;
    let rho = orchard::note::Rho::from_bytes(&rho_bytes).expect("should be valid rho bytes");
    let mut rseed_bytes = [0u8; 32];
    reader.read_exact(&mut rseed_bytes)?;
    let rseed = orchard::note::RandomSeed::from_bytes(rseed_bytes, &rho)
        .expect("should be valid random seed bytes");

    let mut memo_bytes = [0u8; 512];
    reader.read_exact(&mut memo_bytes)?;
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
        note: orchard::note::Note::from_parts(recipient, value, rho, rseed, note_version)
            .expect("should be a valid orchard note"),
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
        let version = reader.read_u8()?;
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
            MAX_SHARDTREE_CHECKPOINTS as usize,
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
                    *shardtree =
                        shardtree::ShardTree::new(store, MAX_SHARDTREE_CHECKPOINTS as usize);
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
        write_with_error_handling!(write_checkpoints, checkpoints);

        // Write cap
        let cap = store.get_cap().expect("Infallible");
        write_with_error_handling!(write_shard, cap);

        *shardtree = shardtree::ShardTree::new(store, MAX_SHARDTREE_CHECKPOINTS as usize);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::witness::ANCHOR_RETENTION_INTERVALS;

    use super::*;

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

    /// The checkpoint set of a synced wallet decomposes into exactly two parts:
    /// [`MAX_SHARDTREE_CHECKPOINTS`] rolling checkpoints, which serve ordinary reorg handling and
    /// near-tip spends, plus [`ANCHOR_RETENTION_INTERVALS`] pinned ZIP 318 grid boundaries,
    /// which serve pool crossings.
    ///
    /// The two parts are disjoint and independently bounded: pinning a boundary must not
    /// consume a rolling slot (that would shrink the reorg window), and the rolling budget must
    /// not displace a boundary (that would break crossings).
    #[test]
    fn checkpoint_set_is_reorg_window_plus_pinned_boundaries() {
        use crate::shardtree_ext::ShardTreeExt as _;
        use crate::witness::{anchor_retention_window, repin_anchor_checkpoints};
        use zcash_client_backend::data_api::anchor_retention::{
            AnchorRetention, AnchorRetentionInterval,
        };

        const TIP: u32 = 100_000;
        const INTERVAL: u32 = 144;
        let policy = AnchorRetention::new(
            BlockHeight::from_u32(90_000),
            AnchorRetentionInterval::default(),
        );
        let mut shard_trees = ShardTrees::new();

        for height in (TIP - 2000)..=TIP {
            let height = BlockHeight::from_u32(height);
            let window = anchor_retention_window(&policy, height);
            repin_anchor_checkpoints(&policy, &window, shard_trees.orchard.store_mut());
            shard_trees
                .orchard
                .append_checkpoint(height)
                .expect("infallible");
        }

        let store = shard_trees.orchard.store();
        let total = store.checkpoint_count().expect("infallible");
        let pinned_ids = store.retained_checkpoints().expect("infallible");
        let mut pinned = Vec::new();
        let mut rolling = Vec::new();
        store
            .for_each_checkpoint(total, |id, _| {
                if pinned_ids.contains(id) {
                    pinned.push(u32::from(*id));
                } else {
                    rolling.push(u32::from(*id));
                }
                Ok(())
            })
            .expect("infallible");

        assert!(
            pinned.iter().all(|height| height % INTERVAL == 0),
            "every pinned checkpoint must be a grid boundary, got {pinned:?}"
        );
        assert_eq!(rolling.last().copied(), Some(TIP));
        assert_eq!(
            (rolling.len(), pinned.len(), total),
            (
                MAX_SHARDTREE_CHECKPOINTS as usize,
                ANCHOR_RETENTION_INTERVALS as usize,
                (MAX_SHARDTREE_CHECKPOINTS + ANCHOR_RETENTION_INTERVALS) as usize,
            ),
            "(rolling, pinned, total): the pinned boundaries must not be part of the \
             MAX_SHARDTREE_CHECKPOINTS total"
        );

        for height in (TIP - MAX_SHARDTREE_CHECKPOINTS + 1)..=TIP {
            assert!(
                store
                    .get_checkpoint(&BlockHeight::from_u32(height))
                    .expect("infallible")
                    .is_some(),
                "reorg window is missing height {height}"
            );
        }

        assert!(
            rolling
                .iter()
                .all(|height| *height >= TIP - MAX_SHARDTREE_CHECKPOINTS),
            "a rolling checkpoint survived below the reorg window: {rolling:?}"
        );

        let mut bytes = Vec::new();
        shard_trees.write(&mut bytes).expect("write should succeed");
        let reloaded = ShardTrees::read(bytes.as_slice()).expect("read should succeed");
        let reloaded_store = reloaded.orchard.store();
        for boundary in &pinned {
            assert!(
                reloaded_store
                    .get_checkpoint(&BlockHeight::from_u32(*boundary))
                    .expect("infallible")
                    .is_some(),
                "boundary {boundary} lost on reload; a crossing anchored there cannot be built"
            );
        }
        assert_eq!(
            reloaded_store.checkpoint_count().expect("infallible"),
            (MAX_SHARDTREE_CHECKPOINTS + ANCHOR_RETENTION_INTERVALS) as usize
        );
    }

    /// Serialization preserves the checkpoints pruning left instead of imposing a cap.
    #[test]
    fn shardtree_roundtrip_keeps_newest_checkpoints() {
        use crate::shardtree_ext::ShardTreeExt as _;

        let mut shard_trees = ShardTrees::new();

        for height in 1..=150 {
            let height = BlockHeight::from_u32(height);
            shard_trees
                .sapling
                .append_checkpoint(height)
                .expect("infallible");
            shard_trees
                .orchard
                .append_checkpoint(height)
                .expect("infallible");
        }

        let mut bytes = Vec::new();
        shard_trees.write(&mut bytes).expect("write should succeed");
        let roundtripped = ShardTrees::read(bytes.as_slice()).expect("read should succeed");

        let sapling_store = roundtripped.sapling.store();
        let orchard_store = roundtripped.orchard.store();

        let oldest_kept = BlockHeight::from_u32(150 - MAX_SHARDTREE_CHECKPOINTS + 1);
        fn assert_window<S>(store: &S, oldest_kept: BlockHeight)
        where
            S: ShardStore<CheckpointId = BlockHeight, Error = std::convert::Infallible>,
        {
            assert_eq!(
                store.checkpoint_count().expect("infallible"),
                MAX_SHARDTREE_CHECKPOINTS as usize
            );
            assert_eq!(
                store.min_checkpoint_id().expect("infallible"),
                Some(oldest_kept)
            );
            assert_eq!(
                store.max_checkpoint_id().expect("infallible"),
                Some(BlockHeight::from_u32(150))
            );
            assert!(
                store
                    .get_checkpoint(&(oldest_kept - 1))
                    .expect("infallible")
                    .is_none()
            );
        }
        assert_window(sapling_store, oldest_kept);
        assert_window(orchard_store, oldest_kept);
    }

    /// A pinned anchor checkpoint survives serialization even once it has aged out of the
    /// rolling window. The pinned set itself is not persisted, being re-derived at the start of
    /// every sync.
    #[test]
    fn shardtree_roundtrip_keeps_pinned_anchor_checkpoints() {
        use crate::shardtree_ext::ShardTreeExt as _;

        let pinned = BlockHeight::from_u32(24);
        let mut shard_trees = ShardTrees::new();

        shard_trees
            .sapling
            .store_mut()
            .add_retained_checkpoint(pinned)
            .expect("infallible");
        for height in 1..=150 {
            shard_trees
                .sapling
                .append_checkpoint(BlockHeight::from_u32(height))
                .expect("infallible");
        }

        assert!(
            shard_trees
                .sapling
                .store()
                .get_checkpoint(&pinned)
                .expect("infallible")
                .is_some(),
            "pruning must not evict a pinned checkpoint"
        );

        let mut bytes = Vec::new();
        shard_trees.write(&mut bytes).expect("write should succeed");
        let roundtripped = ShardTrees::read(bytes.as_slice()).expect("read should succeed");

        let sapling_store = roundtripped.sapling.store();
        assert!(
            sapling_store
                .get_checkpoint(&pinned)
                .expect("infallible")
                .is_some(),
            "serialization must not evict a pinned checkpoint"
        );
        assert_eq!(
            sapling_store.checkpoint_count().expect("infallible"),
            MAX_SHARDTREE_CHECKPOINTS as usize + 1
        );
        assert!(
            sapling_store
                .retained_checkpoints()
                .expect("infallible")
                .is_empty()
        );
    }
}

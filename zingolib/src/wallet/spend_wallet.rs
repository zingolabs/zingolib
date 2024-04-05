use crate::wallet::data::TransactionRecord;
use crate::wallet::notes::NoteInterface;
use crate::wallet::notes::ShieldedNoteInterface;

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

use super::data::{WitnessTrees, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL};
use super::keys::unified::Fvk as _;
use super::keys::unified::{Capability, WalletCapability};
use super::traits::Recipient;
use super::traits::{DomainWalletExt, SpendableNote};

use super::LightWallet;
use super::{
    data::{BlockData, WalletZecPriceInfo},
    message::Message,
    transaction_context::TransactionContext,
    transactions::TxMapAndMaybeTrees,
};

impl LightWallet {
    pub(crate) async fn initiate_witness_trees(&self, trees: TreeState) {
        let (legacy_sapling_frontier, legacy_orchard_frontier) =
            crate::data::witness_trees::get_legacy_frontiers(trees);
        if let Some(ref mut trees) = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await
            .witness_trees
        {
            trees.insert_all_frontier_nodes(legacy_sapling_frontier, legacy_orchard_frontier)
        };
    }
    pub async fn ensure_witness_tree_not_above_wallet_blocks(&self) {
        let last_synced_height = self.last_synced_height().await;
        let mut txmds_writelock = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        if let Some(ref mut trees) = txmds_writelock.witness_trees {
            trees
                .witness_tree_sapling
                .truncate_removing_checkpoint(&BlockHeight::from(last_synced_height as u32))
                .expect("Infallible");
            trees
                .witness_tree_orchard
                .truncate_removing_checkpoint(&BlockHeight::from(last_synced_height as u32))
                .expect("Infallible");
            trees.add_checkpoint(BlockHeight::from(last_synced_height as u32));
        }
    }

    pub async fn has_any_empty_commitment_trees(&self) -> bool {
        self.transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees
            .as_ref()
            .is_some_and(|trees| {
                trees
                    .witness_tree_orchard
                    .max_leaf_position(0)
                    .unwrap()
                    .is_none()
                    || trees
                        .witness_tree_sapling
                        .max_leaf_position(0)
                        .unwrap()
                        .is_none()
            })
    }

    pub(super) async fn get_orchard_anchor(
        &self,
        tree: &ShardTree<
            MemoryShardStore<MerkleHashOrchard, BlockHeight>,
            COMMITMENT_TREE_LEVELS,
            MAX_SHARD_LEVEL,
        >,
    ) -> Result<orchard::Anchor, ShardTreeError<Infallible>> {
        Ok(orchard::Anchor::from(tree.root_at_checkpoint_depth(
            self.transaction_context.config.reorg_buffer_offset as usize,
        )?))
    }
    pub(super) async fn get_sapling_anchor(
        &self,
        tree: &ShardTree<
            MemoryShardStore<sapling_crypto::Node, BlockHeight>,
            COMMITMENT_TREE_LEVELS,
            MAX_SHARD_LEVEL,
        >,
    ) -> Result<sapling_crypto::Anchor, ShardTreeError<Infallible>> {
        Ok(sapling_crypto::Anchor::from(
            tree.root_at_checkpoint_depth(
                self.transaction_context.config.reorg_buffer_offset as usize,
            )?,
        ))
    }

    /// Determines the target height for a transaction, and the offset from which to
    /// select anchors, based on the current synchronised block chain.
    pub(super) async fn get_target_height_and_anchor_offset(&self) -> Option<(u32, usize)> {
        let range = {
            let blocks = self.blocks.read().await;
            (
                blocks.last().map(|block| block.height as u32),
                blocks.first().map(|block| block.height as u32),
            )
        };
        match range {
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
}

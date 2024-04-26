//! implements WalletCommitmentTrees on WitnessTrees

use std::convert::Infallible;

use incrementalmerkletree::Address;
use shardtree::{error::ShardTreeError, ShardTree};
use zcash_client_backend::data_api::{
    chain::CommitmentTreeRoot, WalletCommitmentTrees, ORCHARD_SHARD_HEIGHT, SAPLING_SHARD_HEIGHT,
};

use crate::data::witness_trees::{OrchStore, SapStore};

use super::WitnessTrees;

impl WalletCommitmentTrees for WitnessTrees {
    // review! could this be a zingolib error?
    type Error = Infallible;

    type SaplingShardStore<'a> = SapStore;

    // this code was copied from zcash_client_backend example
    fn with_sapling_tree_mut<F, A, E>(&mut self, mut callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::SaplingShardStore<'a>,
                { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
                { zcash_client_backend::data_api::SAPLING_SHARD_HEIGHT },
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        callback(&mut self.witness_tree_sapling)
    }

    // this code was copied from zcash_client_backend example
    fn put_sapling_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<sapling_crypto::Node>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_sapling_tree_mut(|t| {
            for (root, i) in roots.iter().zip(0u64..) {
                let root_addr = Address::from_parts(SAPLING_SHARD_HEIGHT.into(), start_index + i);
                t.insert(root_addr, *root.root_hash())?;
            }
            Ok::<_, ShardTreeError<Self::Error>>(())
        })?;

        Ok(())
    }

    type OrchardShardStore<'a> = OrchStore;

    // this code was copied from zcash_client_backend example
    fn with_orchard_tree_mut<F, A, E>(&mut self, mut callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::OrchardShardStore<'a>,
                { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                { zcash_client_backend::data_api::ORCHARD_SHARD_HEIGHT },
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        callback(&mut self.witness_tree_orchard)
    }

    // this code was copied from zcash_client_backend example
    fn put_orchard_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_orchard_tree_mut(|t| {
            for (root, i) in roots.iter().zip(0u64..) {
                let root_addr = Address::from_parts(ORCHARD_SHARD_HEIGHT.into(), start_index + i);
                t.insert(root_addr, *root.root_hash())?;
            }
            Ok::<_, ShardTreeError<Self::Error>>(())
        })?;

        Ok(())
    }
}

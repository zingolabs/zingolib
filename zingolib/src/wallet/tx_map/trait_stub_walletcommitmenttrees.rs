//! implements WalletCommitmentTrees on WitnessTrees

use std::convert::Infallible;

use shardtree::{error::ShardTreeError, ShardTree};
use zcash_client_backend::data_api::{chain::CommitmentTreeRoot, WalletCommitmentTrees};

use crate::data::witness_trees::OrchStore;
use crate::data::witness_trees::SapStore;
use crate::wallet::tx_map::TxMap;

impl WalletCommitmentTrees for TxMap {
    // review! could this be a zingolib error?
    type Error = Infallible;

    type SaplingShardStore<'a> = SapStore;

    // this code was copied from zcash_client_backend example
    fn with_sapling_tree_mut<F, A, E>(&mut self, callback: F) -> Result<A, E>
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
        self.witness_trees_mut()
            .expect("this will crash if called on a Watch")
            .with_sapling_tree_mut(callback)
    }

    // this code was copied from zcash_client_backend example
    fn put_sapling_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<sapling_crypto::Node>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.witness_trees_mut()
            .expect("this will crash if called on a Watch")
            .put_sapling_subtree_roots(start_index, roots)
    }

    type OrchardShardStore<'a> = OrchStore;

    // this code was copied from zcash_client_backend example
    fn with_orchard_tree_mut<F, A, E>(&mut self, callback: F) -> Result<A, E>
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
        self.witness_trees_mut()
            .expect("this will crash if called on a Watch")
            .with_orchard_tree_mut(callback)
    }

    // this code was copied from zcash_client_backend example
    fn put_orchard_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.witness_trees_mut()
            .expect("this will crash if called on a Watch")
            .put_orchard_subtree_roots(start_index, roots)
    }
}

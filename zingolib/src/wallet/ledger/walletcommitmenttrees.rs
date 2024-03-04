use std::convert::Infallible;

use shardtree::{error::ShardTreeError, ShardTree};
use zcash_client_backend::{
    data_api::{
        chain::CommitmentTreeRoot, WalletCommitmentTrees, WalletRead, ORCHARD_SHARD_HEIGHT,
        SAPLING_SHARD_HEIGHT,
    },
    keys::UnifiedFullViewingKey,
};

use super::ZingoLedger;
use crate::{
    error::ZingoLibError,
    wallet::data::{OrchStore, SapStore},
    SaplingParams,
};

impl WalletCommitmentTrees for &ZingoLedger {
    type Error = Infallible;

    type SaplingShardStore<'a> = SapStore;

    fn with_sapling_tree_mut<F, A, E>(&mut self, callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::SaplingShardStore<'a>,
                { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
                SAPLING_SHARD_HEIGHT,
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        unimplemented!();
    }

    fn put_sapling_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<sapling_crypto::Node>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        unimplemented!();
    }

    type OrchardShardStore<'a> = OrchStore;

    fn with_orchard_tree_mut<F, A, E>(&mut self, callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::OrchardShardStore<'a>,
                { ORCHARD_SHARD_HEIGHT * 2 },
                ORCHARD_SHARD_HEIGHT,
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        unimplemented!();
    }

    fn put_orchard_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        unimplemented!();
    }
}

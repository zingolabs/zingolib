#![forbid(unsafe_code)]

use zl_common::chain;
use zl_netutils::{Indexer, TransparentIndexer, wire};

use crate::protocol;

pub trait SyncIndexer:
    Clone + Indexer<TreeState = chain::TreeStateReply> + TransparentIndexer + Sync + Send + 'static
{
}

impl<C> SyncIndexer for C where
    C: Clone
        + Indexer<TreeState = chain::TreeStateReply>
        + TransparentIndexer
        + Sync
        + Send
        + 'static
{
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zcash;

impl wire::Vocabulary for Zcash {
    type TreeState = chain::TreeStateReply;

    fn tree_state(reply: protocol::TreeState) -> Self::TreeState {
        chain::TreeStateReply {
            height: chain::TreeStateReply::block_height(reply.height),
            hash: chain::TreeStateReply::block_hash(&reply.hash),
            sapling_tree: chain::Tree::from_hex(&reply.sapling_tree),
            orchard_tree: chain::Tree::from_hex(&reply.orchard_tree),
            ironwood_tree: chain::Tree::from_hex(&reply.ironwood_tree),
        }
    }
}

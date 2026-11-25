#![forbid(unsafe_code)]

use zl_common::chain;
use zl_netutils::wire;

use crate::protocol;

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

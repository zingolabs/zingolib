use zl_common::chain;
use zl_netutils::{tree_state, wire};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zcash;

impl wire::Vocabulary for Zcash {
    type TreeState = chain::TreeStateReply;

    fn tree_state(wire_parts: tree_state::WireParts) -> Self::TreeState {
        chain::TreeStateReply {
            height: chain::TreeStateReply::block_height(wire_parts.height),
            hash: chain::TreeStateReply::block_hash(&wire_parts.hash_hex),
            sapling_tree: chain::Tree::from_hex(&wire_parts.sapling_tree_hex),
            orchard_tree: chain::Tree::from_hex(&wire_parts.orchard_tree_hex),
            ironwood_tree: chain::Tree::from_hex(&wire_parts.ironwood_tree_hex),
        }
    }
}

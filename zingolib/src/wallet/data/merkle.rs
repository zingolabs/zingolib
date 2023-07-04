use incrementalmerkletree::{Address, Position};
use shardtree::QueryError;

use super::ZingoShardStoreError;

/// Returns a Vec containing the address specified, and all of it's descendents
pub(crate) fn address_and_descendents(addr: &Address) -> Vec<Address> {
    if let Some((left, right)) = addr.children() {
        let mut descendents = address_and_descendents(&left);
        descendents.extend_from_slice(&address_and_descendents(&right));
        descendents
    } else {
        vec![*addr]
    }
}

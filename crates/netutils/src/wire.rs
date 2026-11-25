#![forbid(unsafe_code)]

use crate::lightwallet_protocol;

pub trait Vocabulary {
    type TreeState: Send + 'static;

    fn tree_state(reply: lightwallet_protocol::TreeState) -> Self::TreeState;
}

#[derive(Debug, Clone, Copy)]
pub struct Generated;

impl Vocabulary for Generated {
    type TreeState = lightwallet_protocol::TreeState;

    fn tree_state(reply: lightwallet_protocol::TreeState) -> Self::TreeState {
        reply
    }
}

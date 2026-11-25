#![forbid(unsafe_code)]

use crate::lightwallet_protocol;

pub trait Vocabulary {
    type TreeState;
    type Transaction;

    fn tree_state(reply: lightwallet_protocol::TreeState) -> Self::TreeState;

    fn transaction(reply: lightwallet_protocol::RawTransaction) -> Self::Transaction;
}

#[derive(Debug, Clone, Copy)]
pub struct Generated;

impl Vocabulary for Generated {
    type TreeState = lightwallet_protocol::TreeState;
    type Transaction = lightwallet_protocol::RawTransaction;

    fn tree_state(reply: lightwallet_protocol::TreeState) -> Self::TreeState {
        reply
    }

    fn transaction(reply: lightwallet_protocol::RawTransaction) -> Self::Transaction {
        reply
    }
}

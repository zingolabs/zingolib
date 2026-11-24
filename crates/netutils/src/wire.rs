use crate::tree_state;

pub trait Vocabulary {
    type TreeState;

    fn tree_state(wire_parts: tree_state::WireParts) -> Self::TreeState;
}

use crate::tree_state;

pub trait Vocabulary {
    type TreeState;
    type InvalidTreeState;

    fn tree_state(
        wire_parts: tree_state::WireParts,
    ) -> Result<Self::TreeState, Self::InvalidTreeState>;
}

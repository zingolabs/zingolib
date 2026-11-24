use zl_common::chain;
use zl_netutils::{tree_state, wire};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zcash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OmittedTrees {
    pub sapling: bool,
    pub orchard: bool,
    pub ironwood: bool,
}

impl OmittedTrees {
    pub(crate) fn of(tree_state: &chain::TreeState) -> Self {
        Self {
            sapling: tree_state.sapling_tree.is_empty(),
            orchard: tree_state.orchard_tree.is_empty(),
            ironwood: tree_state.ironwood_tree.is_empty(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{field}")]
pub struct InvalidTreeState {
    pub omitted_trees: OmittedTrees,
    pub field: chain::InvalidTreeState,
}

impl wire::Vocabulary for Zcash {
    type TreeState = chain::TreeState;
    type InvalidTreeState = InvalidTreeState;

    fn tree_state(
        wire_parts: tree_state::WireParts,
    ) -> Result<Self::TreeState, Self::InvalidTreeState> {
        chain::TreeState::from_wire_parts(
            wire_parts.height,
            &wire_parts.hash_hex,
            &wire_parts.sapling_tree_hex,
            &wire_parts.orchard_tree_hex,
            &wire_parts.ironwood_tree_hex,
        )
        .map_err(|field| InvalidTreeState {
            omitted_trees: OmittedTrees {
                sapling: wire_parts.sapling_tree_hex.is_empty(),
                orchard: wire_parts.orchard_tree_hex.is_empty(),
                ironwood: wire_parts.ironwood_tree_hex.is_empty(),
            },
            field,
        })
    }
}

#[cfg(test)]
pub(crate) fn fetched_tree_state(
    wire_parts: tree_state::WireParts,
) -> Result<chain::TreeState, tree_state::Failure<InvalidTreeState>> {
    <Zcash as wire::Vocabulary>::tree_state(wire_parts).map_err(tree_state::Failure::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEIGHT: u64 = 2_000_000;
    const TREE_HEX: &str = "000000";
    const OMITTED_TREE_HEX: &str = "";
    const NOT_HEX: &str = "zz";

    #[test]
    fn an_invalid_tree_state_still_reports_its_omitted_trees() {
        let invalid = <Zcash as wire::Vocabulary>::tree_state(tree_state::WireParts {
            height: HEIGHT,
            hash_hex: NOT_HEX.to_string(),
            sapling_tree_hex: TREE_HEX.to_string(),
            orchard_tree_hex: TREE_HEX.to_string(),
            ironwood_tree_hex: OMITTED_TREE_HEX.to_string(),
        });

        assert!(matches!(
            invalid,
            Err(InvalidTreeState {
                omitted_trees: OmittedTrees {
                    sapling: false,
                    orchard: false,
                    ironwood: true,
                },
                field: chain::InvalidTreeState::InvalidHashHex(_),
            })
        ));
    }
}

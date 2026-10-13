//! The transaction expiry delta.
//!
//! ZIP 203 sizes the delta in blocks for a wall-clock duration of about 50
//! minutes, and NU7's shorter block spacing rescales it (zingo-adrs
//! zingolib/0056). One rule lives here and every build site asks it, since
//! the builder's own default still predates NU7.

use zcash_primitives::transaction::builder::DEFAULT_TX_EXPIRY_DELTA;
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};

/// ZIP 218's `NU7PoWTargetSpacingRatio`: the factor by which NU7 shortens
/// the target block spacing, 75 seconds to 25.
const NU7_POW_TARGET_SPACING_RATIO: u32 = 3;

/// The expiry delta ZIP 203 recommends from NU7 activation: the 50 minutes
/// [`DEFAULT_TX_EXPIRY_DELTA`] held at the 75-second spacing, in 25-second
/// blocks.
pub const NU7_TX_EXPIRY_DELTA: u32 = DEFAULT_TX_EXPIRY_DELTA * NU7_POW_TARGET_SPACING_RATIO;

/// The expiry delta of a transaction targeting `target_height`:
/// [`NU7_TX_EXPIRY_DELTA`] from the NU7 activation, [`DEFAULT_TX_EXPIRY_DELTA`]
/// below it and on a chain that never activates NU7.
pub fn tx_expiry_delta(chain: &impl Parameters, target_height: BlockHeight) -> u32 {
    let nu7_active = chain
        .activation_height(NetworkUpgrade::Nu7)
        .is_some_and(|activation| target_height >= activation);
    if nu7_active {
        NU7_TX_EXPIRY_DELTA
    } else {
        DEFAULT_TX_EXPIRY_DELTA
    }
}

/// The expiry height of a transaction targeting `target_height`.
pub fn tx_expiry_height(chain: &impl Parameters, target_height: BlockHeight) -> BlockHeight {
    target_height + tx_expiry_delta(chain, target_height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChainType;
    use crate::testutils::mock_activation_heights_with;

    const ACTIVATION: u32 = 10;

    fn chain_activating_nu7_at(height: Option<u32>) -> ChainType {
        ChainType::Regtest(mock_activation_heights_with(|era| era.set_nu7(height)))
    }

    /// The delta flips at the activation height itself, since a transaction
    /// targeting the activation block is validated under NU7.
    #[test]
    fn the_delta_triples_from_the_activation_height() {
        let chain = chain_activating_nu7_at(Some(ACTIVATION));
        assert_eq!(
            tx_expiry_delta(&chain, BlockHeight::from_u32(ACTIVATION - 1)),
            DEFAULT_TX_EXPIRY_DELTA
        );
        assert_eq!(
            tx_expiry_delta(&chain, BlockHeight::from_u32(ACTIVATION)),
            NU7_TX_EXPIRY_DELTA
        );
        assert_eq!(
            tx_expiry_height(&chain, BlockHeight::from_u32(ACTIVATION)),
            BlockHeight::from_u32(ACTIVATION + NU7_TX_EXPIRY_DELTA)
        );
    }

    /// A chain that never activates NU7 keeps the pre-NU7 delta at every height.
    #[test]
    fn a_chain_without_nu7_keeps_the_default_delta() {
        let chain = chain_activating_nu7_at(None);
        assert_eq!(
            tx_expiry_delta(&chain, BlockHeight::from_u32(u32::MAX / 2)),
            DEFAULT_TX_EXPIRY_DELTA
        );
    }
}

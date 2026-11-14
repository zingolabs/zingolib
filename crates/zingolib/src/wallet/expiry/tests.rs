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
        BLOSSOM_DEFAULT_TX_EXPIRY_DELTA
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
        BLOSSOM_DEFAULT_TX_EXPIRY_DELTA
    );
}

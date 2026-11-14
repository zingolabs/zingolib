use zcash_protocol::consensus::{MAIN_NETWORK, Parameters as _};

use super::*;

const LAST_KNOWN_CHAIN_HEIGHT: u32 = 3_000_000;
const REORG_START_HEIGHT: BlockHeight =
    BlockHeight::from_u32(LAST_KNOWN_CHAIN_HEIGHT - MAX_REORG_ALLOWANCE + 1);
const BLOCKS_MINED_BELOW_REORG_START: u32 = 500;
const BLOCKS_MINED_ABOVE_SAPLING_ACTIVATION: u32 = 10;

fn start_height(transparent_scan_floor: Option<BlockHeight>) -> BlockHeight {
    discovery_start_height(
        &MAIN_NETWORK,
        BlockHeight::from_u32(LAST_KNOWN_CHAIN_HEIGHT),
        transparent_scan_floor,
    )
}

/// A wallet saved before the transparent scan floor was stored has no floor. The search covers the blocks that
/// may have been re-orged.
#[test]
fn discovery_starts_at_reorg_allowance_with_no_floor() {
    assert_eq!(start_height(None), REORG_START_HEIGHT);
}

/// Fewer blocks were mined during the previous sync session than may have been re-orged.
#[test]
fn discovery_starts_at_reorg_allowance_with_floor_above() {
    for floor in [
        REORG_START_HEIGHT,
        BlockHeight::from_u32(LAST_KNOWN_CHAIN_HEIGHT),
    ] {
        assert_eq!(start_height(Some(floor)), REORG_START_HEIGHT);
    }
}

/// More blocks were mined during the previous sync session than may have been re-orged. Any of them may still be
/// unscanned, so the search starts from the first block mined during that session.
#[test]
fn discovery_starts_above_floor_below_reorg_allowance() {
    let floor = REORG_START_HEIGHT - BLOCKS_MINED_BELOW_REORG_START;

    assert_eq!(start_height(Some(floor)), floor + 1);
}

#[test]
fn discovery_starts_at_sapling_activation_at_the_lowest() {
    let sapling_activation_height = MAIN_NETWORK
        .activation_height(consensus::NetworkUpgrade::Sapling)
        .unwrap();

    assert_eq!(
        discovery_start_height(
            &MAIN_NETWORK,
            sapling_activation_height + BLOCKS_MINED_ABOVE_SAPLING_ACTIVATION,
            None
        ),
        sapling_activation_height
    );
}

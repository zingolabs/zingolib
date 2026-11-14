use zcash_primitives::transaction::TxId;
use zl_netutils::lightwallet_protocol::{ChainMetadata, CompactTx};

use super::*;

fn wallet_block_with_ironwood_size(size: u32) -> WalletBlock {
    WalletBlock::from_parts(
        BlockHeight::from_u32(100),
        BlockHash([1; 32]),
        BlockHash([0; 32]),
        0,
        vec![],
        TreeBounds {
            sapling_initial_tree_size: 10,
            sapling_final_tree_size: 10,
            orchard_initial_tree_size: 10,
            orchard_final_tree_size: 10,
            ironwood_initial_tree_size: size,
            ironwood_final_tree_size: size,
        },
    )
}

fn compact_block_with_ironwood_size(size: u32) -> CompactBlock {
    CompactBlock {
        height: 100,
        hash: vec![1; 32],
        prev_hash: vec![0; 32],
        time: 0,
        header: vec![],
        vtx: vec![],
        chain_metadata: Some(ChainMetadata {
            sapling_commitment_tree_size: 10,
            orchard_commitment_tree_size: 10,
            ironwood_commitment_tree_size: size,
        }),
    }
}

/// A server actively serving ironwood metadata (nonzero) that disagrees
/// with the wallet's own nonzero calculation is corruption, not the
/// known "server does not report the ironwood tree size" case (metadata
/// zero).
/// Validation must reject it exactly as it does for sapling and orchard.
#[test]
fn nonzero_ironwood_tree_size_mismatch_is_rejected() {
    let compact_block = compact_block_with_ironwood_size(999);
    let wallet_block = wallet_block_with_ironwood_size(7);
    assert!(matches!(
        check_tree_size(&compact_block, &wallet_block),
        Err(ScanError::IncorrectTreeSize {
            height: _,
            shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
            ..
        })
    ));
}

/// A placeholder action that parses but decrypts to nothing: enough to
/// be counted by the scanner without building a real note.
fn compact_ironwood_action() -> CompactOrchardAction {
    CompactOrchardAction {
        nullifier: vec![0; 32],
        cmx: vec![0; 32],
        ephemeral_key: vec![0; 32],
        ciphertext: vec![0; 52],
    }
}

fn block_with_served_ironwood_actions(
    action_count: usize,
    metadata_ironwood_size: u32,
) -> CompactBlock {
    CompactBlock {
        height: 100,
        hash: vec![1; 32],
        prev_hash: vec![0; 32],
        time: 0,
        header: vec![],
        vtx: vec![CompactTx {
            index: 0,
            txid: vec![3; 32],
            fee: 0,
            spends: vec![],
            outputs: vec![],
            actions: vec![],
            ironwood_actions: (0..action_count)
                .map(|_| compact_ironwood_action())
                .collect(),
            vin: vec![],
            vout: vec![],
        }],
        chain_metadata: Some(ChainMetadata {
            sapling_commitment_tree_size: 10,
            orchard_commitment_tree_size: 10,
            ironwood_commitment_tree_size: metadata_ironwood_size,
        }),
    }
}

fn initial_scan_data(ironwood_initial_tree_size: u32) -> InitialScanData {
    InitialScanData {
        start_seam_block: None,
        end_seam_block: None,
        sapling_initial_tree_size: 10,
        orchard_initial_tree_size: 10,
        ironwood_initial_tree_size,
    }
}

/// A block's calculated tree size is the scan baseline plus every output
/// served on the wire, for ironwood exactly as for orchard.
#[test]
fn ironwood_calculated_size_counts_served_actions() {
    let scan_data = scan_compact_blocks(
        vec![block_with_served_ironwood_actions(5, 105)],
        &zcash_protocol::consensus::MAIN_NETWORK,
        &HashMap::new(),
        initial_scan_data(100),
        100,
        HashMap::new(),
        HashMap::new(),
        10,
        BlockHeight::from_u32(0),
        &BTreeSet::new(),
    )
    .unwrap();

    let tree_bounds = scan_data
        .wallet_blocks
        .values()
        .next()
        .unwrap()
        .tree_bounds();
    assert_eq!(tree_bounds.ironwood_initial_tree_size, 100);
    assert_eq!(tree_bounds.ironwood_final_tree_size, 105);
}

/// A leaf checkpoint is placed on the last note commitment of a checkpoint height and on no other block.
#[test]
fn checkpoints_are_placed_on_the_checkpoint_heights_only() {
    let first = block_with_served_ironwood_actions(5, 105);
    let mut second = block_with_served_ironwood_actions(5, 110);
    second.height = 101;
    second.hash = vec![2; 32];
    second.prev_hash = first.hash.clone();
    second.vtx[0].txid = vec![4; 32];

    let scan_data = scan_compact_blocks(
        vec![first, second],
        &zcash_protocol::consensus::MAIN_NETWORK,
        &HashMap::new(),
        initial_scan_data(100),
        100,
        HashMap::new(),
        HashMap::new(),
        10,
        BlockHeight::from_u32(0),
        &BTreeSet::from([BlockHeight::from_u32(101)]),
    )
    .unwrap();

    let retentions = scan_data
        .witness_data
        .ironwood_leaves_and_retentions
        .iter()
        .map(|(_, retention)| *retention)
        .collect::<Vec<_>>();
    assert_eq!(retentions.len(), 10);
    assert_eq!(retentions[4], Retention::Ephemeral);
    assert_eq!(
        retentions[9],
        Retention::Checkpoint {
            id: BlockHeight::from_u32(101),
            marking: Marking::None,
        }
    );
}

/// A baseline understating the true tree size below the range must fail
/// the scan at the first served action, escaping the metadata-zero
/// tolerance. Note positions and witnesses derive from the same
/// baseline, so scanning past the mismatch would corrupt them silently.
#[test]
fn understated_ironwood_baseline_is_rejected() {
    let result = scan_compact_blocks(
        vec![block_with_served_ironwood_actions(5, 105)],
        &zcash_protocol::consensus::MAIN_NETWORK,
        &HashMap::new(),
        initial_scan_data(0),
        100,
        HashMap::new(),
        HashMap::new(),
        10,
        BlockHeight::from_u32(0),
        &BTreeSet::new(),
    );

    assert!(matches!(
        result,
        Err(ScanError::IncorrectTreeSize {
            height: _,
            shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
            block_metadata_size: 105,
            calculated_size: 5,
        })
    ));
}

fn block_with_served_orchard_actions(
    action_count: usize,
    metadata_orchard_size: u32,
) -> CompactBlock {
    let mut compact_block = block_with_served_ironwood_actions(0, 10);
    compact_block.vtx[0].actions = (0..action_count)
        .map(|_| compact_ironwood_action())
        .collect();
    compact_block
        .chain_metadata
        .as_mut()
        .expect("the helper always builds metadata")
        .orchard_commitment_tree_size = metadata_orchard_size;
    compact_block
}

/// A wallet whose history never tracked a pool records no tree for it,
/// so its blocks report zero where the chain reports a populated tree.
/// That is the wallet's record failing to account for the chain, and it
/// must be named rather than tolerated: tolerating it leaves the notes
/// in that history undetected and the balance understating the wallet.
#[test]
fn untracked_pool_history_is_rejected() {
    let compact_block = compact_block_with_ironwood_size(1354);
    let wallet_block = wallet_block_with_ironwood_size(0);

    assert!(matches!(
        check_tree_size(&compact_block, &wallet_block),
        Err(ScanError::IncorrectTreeSize {
            height: _,
            shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
            block_metadata_size: 1354,
            calculated_size: 0,
        })
    ));
}

/// The same reasoning, for a pool that activated long before this build:
/// nothing about it depends on which pool is newest, on an activation
/// height, or on a network.
#[test]
fn untracked_pool_history_is_rejected_for_every_pool() {
    let compact_block = block_with_served_orchard_actions(0, 1354);
    let wallet_block = wallet_block_with_ironwood_size(10);

    assert!(matches!(
        check_tree_size(&compact_block, &wallet_block),
        Err(ScanError::IncorrectTreeSize {
            height: _,
            shielded_protocol: PoolType::Shielded(ShieldedPool::Orchard),
            block_metadata_size: 1354,
            calculated_size: 10,
        })
    ));
}

/// A server that does not report a pool's tree size leaves it at zero
/// while the block still carries that pool's outputs. The wallet's own
/// record is not at fault, so rather than reopening the pool's history
/// for a rescan that would never resolve it, the scan fails with an error
/// naming the unreported pool.
#[test]
fn an_unreported_tree_size_fails_the_scan() {
    let compact_block = block_with_served_ironwood_actions(5, 0);
    let wallet_block = wallet_block_with_ironwood_size(5);

    assert!(matches!(
        check_tree_size(&compact_block, &wallet_block),
        Err(ScanError::TreeSizeNotReported {
            shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
            calculated_size: 5,
            ..
        })
    ));
}

/// The wallet's count is cumulative, so against a non-reporting server
/// the mismatch persists onto blocks that serve no outputs of their own.
#[test]
fn an_unreported_tree_size_fails_the_scan_on_blocks_without_outputs() {
    let compact_block = block_with_served_ironwood_actions(0, 0);
    let wallet_block = wallet_block_with_ironwood_size(7);

    assert!(matches!(
        check_tree_size(&compact_block, &wallet_block),
        Err(ScanError::TreeSizeNotReported {
            shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
            calculated_size: 7,
            ..
        })
    ));
}

/// Block metadata omits a tree size of zero, as at the ironwood activation
/// block where the tree is still empty. A zero where the wallet also
/// calculates zero is an empty tree, not an unreported one.
#[test]
fn an_empty_tree_is_not_an_unreported_tree_size() {
    let compact_block = block_with_served_ironwood_actions(0, 0);
    let wallet_block = wallet_block_with_ironwood_size(0);

    assert!(check_tree_size(&compact_block, &wallet_block).is_ok());
}

const GAP_LIMIT: u32 = 3;

fn transparent_test_ufvk() -> UnifiedFullViewingKey {
    zcash_keys::keys::UnifiedSpendingKey::from_seed(
        &zcash_protocol::consensus::MAIN_NETWORK,
        &[7; 32],
        AccountId::ZERO,
    )
    .expect("a 32 byte seed derives a spending key")
    .to_unified_full_viewing_key()
}

fn external_address(
    ufvk: &UnifiedFullViewingKey,
    index: u32,
) -> zcash_transparent::address::TransparentAddress {
    use zcash_transparent::keys::IncomingViewingKey as _;

    ufvk.p2pkh()
        .expect("the test key has a transparent component")
        .derive_external_ivk()
        .unwrap()
        .derive_address(zcash_transparent::keys::NonHardenedChildIndex::from_index(index).unwrap())
        .unwrap()
}

/// The address map the scanner expects, keyed by encoded address, for the given external indexes.
fn external_addresses(
    ufvk: &UnifiedFullViewingKey,
    indexes: impl IntoIterator<Item = u32>,
) -> HashMap<String, TransparentAddressId> {
    indexes
        .into_iter()
        .map(|index| {
            (
                keys::transparent::encode_address(
                    &zcash_protocol::consensus::MAIN_NETWORK,
                    external_address(ufvk, index),
                ),
                TransparentAddressId::new(
                    AccountId::ZERO,
                    TransparentScope::External,
                    zcash_transparent::keys::NonHardenedChildIndex::from_index(index).unwrap(),
                ),
            )
        })
        .collect()
}

/// A block with one transaction paying the external address at `index`.
fn block_funding_external_address(
    ufvk: &UnifiedFullViewingKey,
    height: u64,
    index: u32,
) -> CompactBlock {
    let script = Script::from(external_address(ufvk, index).script());
    let mut compact_block = block_with_served_ironwood_actions(0, 10);
    compact_block.height = height;
    compact_block.vtx[0].vout = vec![zl_netutils::lightwallet_protocol::TxOut {
        value: 100_000,
        script_pub_key: script.0.0,
    }];
    compact_block
}

fn scan_block(
    ufvk: &UnifiedFullViewingKey,
    compact_block: CompactBlock,
    inuse_addresses: &HashMap<String, TransparentAddressId>,
    gap_addresses: &HashMap<String, TransparentAddressId>,
) -> ScanData {
    scan_block_above_floor(
        ufvk,
        compact_block,
        inuse_addresses,
        gap_addresses,
        BlockHeight::from_u32(0),
    )
}

fn scan_block_above_floor(
    ufvk: &UnifiedFullViewingKey,
    compact_block: CompactBlock,
    inuse_addresses: &HashMap<String, TransparentAddressId>,
    gap_addresses: &HashMap<String, TransparentAddressId>,
    transparent_scan_floor: BlockHeight,
) -> ScanData {
    scan_compact_blocks(
        vec![compact_block],
        &zcash_protocol::consensus::MAIN_NETWORK,
        &HashMap::from([(AccountId::ZERO, ufvk.clone())]),
        initial_scan_data(10),
        100,
        inuse_addresses.clone(),
        gap_addresses.clone(),
        GAP_LIMIT,
        transparent_scan_floor,
        &BTreeSet::new(),
    )
    .unwrap()
}

/// Funding a gap address moves it, and every gap address below it, to in-use, and derives new gap addresses
/// so the gap limit is kept past the highest address in use. The new gap addresses are then used to scan
/// the next block, so an address just past the gap is not found and the last address in the gap is.
#[test]
fn gap_addresses_move_to_inuse_and_are_replaced() {
    let ufvk = transparent_test_ufvk();
    let mut inuse_addresses = external_addresses(&ufvk, [0]);
    let mut gap_addresses = external_addresses(&ufvk, 1..=3);

    // fund index 3 only: the unfunded 1 and 2 move to in-use with it
    let block_a = block_funding_external_address(&ufvk, 100, 3);
    let scan_data = scan_block(&ufvk, block_a, &inuse_addresses, &gap_addresses);
    assert_eq!(
        scan_data.new_transparent_inuse_addresses,
        external_addresses(&ufvk, 1..=3)
    );
    assert_eq!(
        scan_data.new_transparent_gap_addresses,
        external_addresses(&ufvk, 4..=6)
    );
    assert_eq!(scan_data.decrypted_scan_targets.len(), 1);
    inuse_addresses.extend(scan_data.new_transparent_inuse_addresses);
    gap_addresses = scan_data.new_transparent_gap_addresses;

    // fund index 7, one past the gap: nothing is found and the gap is unchanged
    let block_b = block_funding_external_address(&ufvk, 101, 7);
    let scan_data = scan_block(&ufvk, block_b, &inuse_addresses, &gap_addresses);
    assert!(scan_data.new_transparent_inuse_addresses.is_empty());
    assert!(scan_data.new_transparent_gap_addresses.is_empty());
    assert!(scan_data.decrypted_scan_targets.is_empty());

    // fund index 6, the last address in the gap: 4 to 6 move to in-use
    let block_c = block_funding_external_address(&ufvk, 102, 6);
    let scan_data = scan_block(&ufvk, block_c, &inuse_addresses, &gap_addresses);
    assert_eq!(
        scan_data.new_transparent_inuse_addresses,
        external_addresses(&ufvk, 4..=6)
    );
    assert_eq!(
        scan_data.new_transparent_gap_addresses,
        external_addresses(&ufvk, 7..=9)
    );
    assert_eq!(scan_data.decrypted_scan_targets.len(), 1);
    inuse_addresses.extend(scan_data.new_transparent_inuse_addresses);
    assert_eq!(inuse_addresses, external_addresses(&ufvk, 0..=6));
}

/// Only the gap addresses derived by the scan are returned. The gap addresses above the highest address found
/// in use are already held by the scanner.
#[test]
fn new_gap_addresses_are_the_derived_addresses() {
    let ufvk = transparent_test_ufvk();
    let inuse_addresses = external_addresses(&ufvk, [0]);
    let gap_addresses = external_addresses(&ufvk, 1..=3);

    let block = block_funding_external_address(&ufvk, 100, 1);
    let scan_data = scan_block(&ufvk, block, &inuse_addresses, &gap_addresses);
    assert_eq!(
        scan_data.new_transparent_inuse_addresses,
        external_addresses(&ufvk, [1])
    );
    assert_eq!(
        scan_data.new_transparent_gap_addresses,
        external_addresses(&ufvk, [4])
    );
}

/// A gap address derived by the scan and then found in use by the same scan is returned as in-use only.
#[test]
fn derived_gap_addresses_found_in_use_are_new_inuse_addresses() {
    let ufvk = transparent_test_ufvk();
    let inuse_addresses = external_addresses(&ufvk, [0]);
    let gap_addresses = external_addresses(&ufvk, 1..=3);

    // index 5 is only a gap address once funding index 3 has derived 4 to 6
    let mut block = block_funding_external_address(&ufvk, 100, 3);
    let funding_past_gap = block_funding_external_address(&ufvk, 100, 5).vtx.remove(0);
    block.vtx[0].vout.extend(funding_past_gap.vout);
    let scan_data = scan_block(&ufvk, block, &inuse_addresses, &gap_addresses);
    assert_eq!(
        scan_data.new_transparent_inuse_addresses,
        external_addresses(&ufvk, 1..=5)
    );
    assert_eq!(
        scan_data.new_transparent_gap_addresses,
        external_addresses(&ufvk, 6..=8)
    );
}

/// Only the transparent inputs of blocks above the transparent scan floor are collected, as transparent address
/// discovery has already located all relevant transactions at or below the floor.
#[test]
fn outpoints_only_collected_above_transparent_scan_floor() {
    fn block_spending_outpoint(height: u64, hash: u8, prevout_txid: [u8; 32]) -> CompactBlock {
        let mut compact_block = block_with_served_ironwood_actions(0, 10);
        compact_block.height = height;
        compact_block.hash = vec![hash; 32];
        compact_block.prev_hash = vec![hash - 1; 32];
        compact_block.vtx[0].txid = vec![hash + 10; 32];
        compact_block.vtx[0].vin = vec![zl_netutils::lightwallet_protocol::CompactTxIn {
            prevout_txid: prevout_txid.to_vec(),
            prevout_index: 0,
        }];
        compact_block
    }

    let scan_data = scan_compact_blocks(
        vec![
            block_spending_outpoint(100, 1, [20; 32]),
            block_spending_outpoint(101, 2, [21; 32]),
        ],
        &zcash_protocol::consensus::MAIN_NETWORK,
        &HashMap::new(),
        initial_scan_data(10),
        100,
        HashMap::new(),
        HashMap::new(),
        GAP_LIMIT,
        BlockHeight::from_u32(100),
        &BTreeSet::new(),
    )
    .unwrap();

    assert_eq!(
        scan_data.outpoints.into_iter().collect::<Vec<_>>(),
        vec![(
            OutputId::new(TxId::from_bytes([21; 32]), 0),
            ScanTarget::internal(BlockHeight::from_u32(101), TxId::from_bytes([12; 32])),
        )]
    );
}

/// Transparent outputs of blocks at or below the transparent scan floor are not checked against the in-use and
/// gap addresses, as transparent address discovery has already located all relevant transactions at or below the
/// floor. The same blocks are scanned above the floor to show the outputs would otherwise be found.
#[test]
fn transparent_outputs_only_scanned_above_transparent_scan_floor() {
    let ufvk = transparent_test_ufvk();
    let inuse_addresses = external_addresses(&ufvk, [0]);
    let gap_addresses = external_addresses(&ufvk, 1..=3);

    for (index, expected_new_inuse_addresses) in
        [(0, HashMap::new()), (2, external_addresses(&ufvk, 1..=2))]
    {
        let scan_data = scan_block_above_floor(
            &ufvk,
            block_funding_external_address(&ufvk, 100, index),
            &inuse_addresses,
            &gap_addresses,
            BlockHeight::from_u32(100),
        );
        assert!(scan_data.decrypted_scan_targets.is_empty());
        assert!(scan_data.new_transparent_inuse_addresses.is_empty());
        assert!(scan_data.new_transparent_gap_addresses.is_empty());

        let scan_data = scan_block_above_floor(
            &ufvk,
            block_funding_external_address(&ufvk, 100, index),
            &inuse_addresses,
            &gap_addresses,
            BlockHeight::from_u32(99),
        );
        assert_eq!(scan_data.decrypted_scan_targets.len(), 1);
        assert_eq!(
            scan_data.new_transparent_inuse_addresses,
            expected_new_inuse_addresses
        );
    }
}

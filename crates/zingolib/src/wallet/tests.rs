use std::num::NonZeroU32;

use incrementalmerkletree::frontier::CommitmentTree;
use orchard::tree::MerkleHashOrchard;
use zcash_protocol::consensus::BlockHeight;
use zingo_test_vectors::seeds;

use super::LightWallet;
use crate::config::{ChainType, WalletConfig};
use crate::testutils::default_test_wallet_settings;

const MAINNET_SAPLING_ACTIVATION: u32 = 419_200;

fn mainnet_wallet(birthday: u32) -> LightWallet {
    LightWallet::new(
        ChainType::Mainnet,
        WalletConfig::MnemonicPhrase {
            mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded non-zero integer"),
            birthday,
            wallet_settings: default_test_wallet_settings(),
        },
    )
    .expect("a known seed on mainnet constructs a wallet")
}

#[test]
fn restore_below_sapling_activation_starts_at_activation() {
    let wallet = mainnet_wallet(0);
    assert_eq!(
        wallet.birthday(),
        BlockHeight::from_u32(MAINNET_SAPLING_ACTIVATION)
    );
    assert_eq!(
        wallet.birthday_adjusted_from(),
        Some(BlockHeight::from_u32(0))
    );
}

#[test]
fn restore_at_sapling_activation_reports_no_adjustment() {
    let wallet = mainnet_wallet(MAINNET_SAPLING_ACTIVATION);
    assert_eq!(
        wallet.birthday(),
        BlockHeight::from_u32(MAINNET_SAPLING_ACTIVATION)
    );
    assert_eq!(wallet.birthday_adjusted_from(), None);
}

#[test]
fn read_raises_a_stored_birthday_below_sapling_activation() {
    let mut written = mainnet_wallet(MAINNET_SAPLING_ACTIVATION);
    written.birthday = BlockHeight::from_u32(1_000);
    let mut bytes = Vec::new();
    written
        .write(&mut bytes, &ChainType::Mainnet)
        .expect("a wallet writes into a vec");

    let read = LightWallet::read(bytes.as_slice(), ChainType::Mainnet)
        .expect("the bytes this version wrote read back");

    assert_eq!(
        read.birthday(),
        BlockHeight::from_u32(MAINNET_SAPLING_ACTIVATION)
    );
    assert_eq!(
        read.birthday_adjusted_from(),
        Some(BlockHeight::from_u32(1_000))
    );
    assert!(read.save_required);
}

// TODO: move to relevant mod
#[test]
fn anchor_from_tree_works() {
    // These commitment values copied from zcash/orchard, and were originally derived from the bundle
    // data that was generated for testing commitment tree construction in zcash/zcash here.
    // https://github.com/zcash/zcash/blob/ecec1f9769a5e37eb3f7fd89a4fcfb35bc28eed7/src/test/data/merkle_roots_orchard.h

    let commitments = [
        [
            0x68, 0x13, 0x5c, 0xf4, 0x99, 0x33, 0x22, 0x90, 0x99, 0xa4, 0x4e, 0xc9, 0x9a, 0x75,
            0xe1, 0xe1, 0xcb, 0x46, 0x40, 0xf9, 0xb5, 0xbd, 0xec, 0x6b, 0x32, 0x23, 0x85, 0x6f,
            0xea, 0x16, 0x39, 0x0a,
        ],
        [
            0x78, 0x31, 0x50, 0x08, 0xfb, 0x29, 0x98, 0xb4, 0x30, 0xa5, 0x73, 0x1d, 0x67, 0x26,
            0x20, 0x7d, 0xc0, 0xf0, 0xec, 0x81, 0xea, 0x64, 0xaf, 0x5c, 0xf6, 0x12, 0x95, 0x69,
            0x01, 0xe7, 0x2f, 0x0e,
        ],
        [
            0xee, 0x94, 0x88, 0x05, 0x3a, 0x30, 0xc5, 0x96, 0xb4, 0x30, 0x14, 0x10, 0x5d, 0x34,
            0x77, 0xe6, 0xf5, 0x78, 0xc8, 0x92, 0x40, 0xd1, 0xd1, 0xee, 0x17, 0x43, 0xb7, 0x7b,
            0xb6, 0xad, 0xc4, 0x0a,
        ],
        [
            0x9d, 0xdc, 0xe7, 0xf0, 0x65, 0x01, 0xf3, 0x63, 0x76, 0x8c, 0x5b, 0xca, 0x3f, 0x26,
            0x46, 0x60, 0x83, 0x4d, 0x4d, 0xf4, 0x46, 0xd1, 0x3e, 0xfc, 0xd7, 0xc6, 0xf1, 0x7b,
            0x16, 0x7a, 0xac, 0x1a,
        ],
        [
            0xbd, 0x86, 0x16, 0x81, 0x1c, 0x6f, 0x5f, 0x76, 0x9e, 0xa4, 0x53, 0x9b, 0xba, 0xff,
            0x0f, 0x19, 0x8a, 0x6c, 0xdf, 0x3b, 0x28, 0x0d, 0xd4, 0x99, 0x26, 0x16, 0x3b, 0xd5,
            0x3f, 0x53, 0xa1, 0x21,
        ],
    ];
    let mut orchard_tree: CommitmentTree<MerkleHashOrchard, 32> = CommitmentTree::empty();
    for commitment in commitments {
        orchard_tree
            .append(MerkleHashOrchard::from_bytes(&commitment).unwrap())
            .unwrap();
    }
    // This value was produced by the Python test vector generation code implemented here:
    // https://github.com/zcash-hackworks/zcash-test-vectors/blob/f4d756410c8f2456f5d84cedf6dac6eb8c068eed/orchard_merkle_tree.py
    let anchor = [
        0xc8, 0x75, 0xbe, 0x2d, 0x60, 0x87, 0x3f, 0x8b, 0xcd, 0xeb, 0x91, 0x28, 0x2e, 0x64, 0x2e,
        0x0c, 0xc6, 0x5f, 0xf7, 0xd0, 0x64, 0x2d, 0x13, 0x7b, 0x28, 0xcf, 0x28, 0xcc, 0x9c, 0x52,
        0x7f, 0x0e,
    ];
    let anchor = orchard::Anchor::from(MerkleHashOrchard::from_bytes(&anchor).unwrap());
    assert_eq!(orchard::Anchor::from(orchard_tree.root()), anchor);
}

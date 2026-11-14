use std::collections::{BTreeMap, HashMap};

use super::{MigrationSection, RawMigration, WalletFile, WalletFileRef, WalletSettings};
use zcash_protocol::consensus::BlockHeight;
use zl_common::chain::ChainType;
use zl_common::price::PriceList;
use zl_common::shardtree_ext::{DOWNSTREAM_BOUNDARY_PATH, assert_raw_boolean_calls_confined};
use zl_common::sync::SyncState;
use zl_common::wallet::{NullifierMap, ShardTrees};

#[test]
fn accessors_read_what_into_parts_returns() {
    const BIRTHDAY: u32 = 1_000_000;
    const SECTION: [u8; 3] = [7, 8, 9];
    let chain_type = ChainType::Mainnet;

    let mut bytes = Vec::new();
    WalletFileRef {
        chain_type,
        mnemonic: None,
        birthday: BlockHeight::from_u32(BIRTHDAY),
        unified_key_store: &BTreeMap::new(),
        unified_addresses: &BTreeMap::new(),
        transparent_addresses: &BTreeMap::new(),
        wallet_blocks: &BTreeMap::new(),
        wallet_transactions: &HashMap::new(),
        nullifier_map: &NullifierMap::new(),
        outpoint_map: &BTreeMap::new(),
        shard_trees: &mut ShardTrees::new(),
        sync_state: &mut SyncState::new(),
        wallet_settings: &mut WalletSettings::default(),
        price_list: &PriceList::new(),
        migration: Some(&RawMigration(SECTION.to_vec())),
    }
    .write(&mut bytes, &chain_type)
    .expect("writing into an in-memory Vec<u8> never returns an io::Error");
    let read = || {
        WalletFile::<RawMigration>::read(bytes.as_slice(), chain_type)
            .expect("the bytes were written by the writer of this crate")
    };

    let file = read();
    let parts = read().into_parts();

    assert_eq!(file.read_version(), parts.read_version);
    assert_eq!(file.chain_type(), parts.chain_type);
    assert_eq!(file.mnemonic(), parts.mnemonic.as_ref());
    assert_eq!(file.birthday(), parts.birthday);
    assert_eq!(file.wallet_settings(), &parts.wallet_settings);
    assert_eq!(file.migration(), parts.migration.as_ref());
    assert_eq!(parts.migration, Some(RawMigration(SECTION.to_vec())));
}

#[test]
fn raw_migration_round_trips_every_byte() {
    const SECTION_LENGTH: u8 = 32;
    let section = RawMigration((0..SECTION_LENGTH).collect());

    let mut written = Vec::new();
    section
        .write(&mut written)
        .expect("writing into an in-memory Vec<u8> never returns an io::Error");
    let read_back = RawMigration::read(written.as_slice(), ChainType::Mainnet)
        .expect("reading the rest of an in-memory slice never fails");

    assert_eq!(read_back, section);
}

#[test]
fn raw_boolean_calls_route_through_the_wrapper() {
    assert_raw_boolean_calls_confined(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        None,
        DOWNSTREAM_BOUNDARY_PATH,
    );
}

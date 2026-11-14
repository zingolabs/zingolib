use bip0039::Mnemonic;
use pepper_sync::keys::transparent::{self, TransparentAddressId, TransparentScope};
use zcash_transparent::keys::NonHardenedChildIndex;
use zingo_common_components::protocol::ActivationHeights;
use zingo_test_vectors::seeds;

use crate::config::ChainType;
use crate::wallet::LightWallet;
use crate::wallet::keys::unified::{ReceiverSelection, UnifiedAddressId};

fn regtest_wallet(mnemonic_phrase: &str) -> LightWallet {
    crate::testutils::synthetic_wallet::SyntheticWalletBuilder::new(mnemonic_phrase).build()
}

/// Migrated from libtonode `fast::ensure_taddrs_from_old_seeds_work`.
#[test]
fn taddrs_from_old_seeds_stay_stable() {
    let wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
    // The first taddr generated on commit 9e71a14eb424631372fd08503b1bd83ea763c7fb
    assert_eq!(
        wallet.transparent_addresses().values().next().unwrap(),
        "tmFLszfkjgim4zoUMAXpuohnFBAKy99rr2i"
    );
}

/// Migrated from libtonode `fast::address_generation_deterministic_and_coherent`.
#[test]
fn address_generation_deterministic_and_coherent() {
    let seed_phrase = Mnemonic::<bip0039::English>::from_entropy([1; 32])
        .unwrap()
        .to_string();
    let mut wallet = regtest_wallet(&seed_phrase);
    let network = ChainType::Regtest(ActivationHeights::default());

    // The scenario ClientBuilder::build_client generates an extra
    // sapling-only address right after construction; reproduce it so the
    // vector-pinned indices (2, 3) and their diversified derivations
    // match the original integration test exactly.
    wallet
        .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
        .unwrap();

    let (new_address_id, new_address) = wallet
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .unwrap();
    assert_eq!(
        new_address_id,
        UnifiedAddressId {
            account_id: zip32::AccountId::ZERO,
            address_index: 2
        }
    );
    assert!(new_address.has_orchard());
    assert!(new_address.has_sapling());
    assert!(!new_address.has_transparent());
    assert_eq!(
        new_address.encode(&network),
        "\
uregtest1ds3zxwluuzmcwvdxh4wf8xsger96c5yyzqhwzwu7vt85crj4jyf7nsn258rn89g68lvelsjhkqywz8w70wxdg2cmnul4zadukwu2ywezgjwt36\
f06qvre5qdlkqp5fksyy9j5dm0fdwxwptkk04gzt84r5qv0wfdlx250n0gdcdd6e00"
    );

    let (sapling_address_id, sapling_address) = wallet
        .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
        .unwrap();
    assert_eq!(
        sapling_address_id,
        UnifiedAddressId {
            account_id: zip32::AccountId::ZERO,
            address_index: 3
        }
    );
    assert!(!sapling_address.has_orchard());
    assert!(sapling_address.has_sapling());
    assert!(!sapling_address.has_transparent());
    assert_eq!(
        sapling_address.encode(&network),
        "\
uregtest1n22mmna853578fakgx6z6adn24ey5r7wfye8ulhscqc9hvm0rf5czxjuz9te0zzc8j93y35gzw53tdmgz6dtfvlnfmjwl2a84cx5m3fq"
    );

    let (taddress_id, new_taddress) = wallet
        .generate_transparent_address(zip32::AccountId::ZERO, false)
        .unwrap();
    assert_eq!(
        taddress_id,
        TransparentAddressId::new(
            zip32::AccountId::ZERO,
            TransparentScope::External,
            NonHardenedChildIndex::from_index(1).unwrap()
        )
    );
    assert_eq!(
        transparent::encode_address(&network, new_taddress),
        "\
tmQuMoTTjU3GFfTjrhPiBYihbTVfYmPk5Gr"
    );
}

/// The property ADR 0010 bought by moving reservation to apply time, and
/// the one an external caller now depends on: the address it names to a
/// counterparty is the address the next proposal spends through, however
/// many times it asks in between.
#[test]
fn deriving_a_refund_address_answers_with_the_same_index_until_apply() {
    let wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

    let first = wallet
        .derive_refund_addresses(1, zip32::AccountId::ZERO)
        .unwrap();
    let again = wallet
        .derive_refund_addresses(1, zip32::AccountId::ZERO)
        .unwrap();

    assert_eq!(first, again);
    assert_eq!(
        first[0].0,
        TransparentAddressId::new(
            zip32::AccountId::ZERO,
            TransparentScope::Refund,
            NonHardenedChildIndex::ZERO
        )
    );
}

/// Reserving is what moves the index on, so a caller that reserves before
/// naming an address to a counterparty names one the proposal will not
/// use. That was the bug this method was made public to retire.
#[test]
fn reserving_a_refund_address_moves_the_next_derivation_on() {
    let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

    let reserved = wallet
        .generate_refund_addresses(1, zip32::AccountId::ZERO)
        .unwrap();
    let next = wallet
        .derive_refund_addresses(1, zip32::AccountId::ZERO)
        .unwrap();

    assert_eq!(
        reserved[0].0.address_index(),
        NonHardenedChildIndex::ZERO,
        "the first reservation takes index zero"
    );
    assert_eq!(
        next[0].0.address_index(),
        NonHardenedChildIndex::from_index(1).unwrap(),
        "a reserved index is spent, so the next derivation moves past it"
    );
}

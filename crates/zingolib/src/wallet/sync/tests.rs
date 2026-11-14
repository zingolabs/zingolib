use super::*;
use crate::{
    config::{ChainType, WalletConfig},
    mocks::default_zaddr,
    testutils::default_test_wallet_settings,
    wallet::LightWallet,
};
use sapling_crypto::zip32::DiversifiableFullViewingKey;
use zingo_test_vectors::seeds;
use zip32::DiversifierIndex;

fn test_wallet() -> LightWallet {
    LightWallet::new(
        ChainType::Mainnet,
        WalletConfig::MnemonicPhrase {
            mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 419200,
            wallet_settings: default_test_wallet_settings(),
        },
    )
    .unwrap()
}

/// Index 65 536 (`2u128.pow(16)`, the exclusive upper bound of the 16-bit index space) must
/// early-return `Ok(())` without inserting an address.
#[test]
fn add_sapling_address_at_limit_early_returns() {
    let mut wallet = test_wallet();
    let (_, _, addr) = default_zaddr();
    let account_id = zip32::AccountId::try_from(0).unwrap();

    let at_limit = DiversifierIndex::from(65536u32);
    let before = wallet.unified_addresses().len();
    let result = SyncWallet::add_sapling_address(&mut wallet, account_id, addr, at_limit);

    assert!(result.is_ok());
    assert_eq!(
        wallet.unified_addresses().len(),
        before,
        "early return must not insert an address"
    );
}

/// A receiver discovered at an index joins the address stored there without
/// displacing its other receiver or the expiry it was issued with.
#[test]
fn a_discovered_receiver_keeps_the_stored_expiry() {
    let (_, _, sapling) = default_zaddr();
    let spending_key = orchard::keys::SpendingKey::from_bytes([0; 32]).unwrap();
    let orchard =
        orchard::keys::FullViewingKey::from(&spending_key).address_at(0u32, zip32::Scope::External);
    let stored = UnifiedAddress::from_receivers(
        Some(orchard),
        None,
        None,
        Some(BlockHeight::from_u32(3_000_000)),
        Some(1_900_000_000),
    )
    .unwrap();

    let merged = with_discovered_receiver(Some(&stored), None, Some(sapling));

    assert_eq!(merged.orchard(), Some(&orchard));
    assert_eq!(merged.sapling(), Some(&sapling));
    assert_eq!(merged.expiry_height(), stored.expiry_height());
    assert_eq!(merged.expiry_time(), stored.expiry_time());
}

/// An index below the 16-bit limit must pass the guard and insert the sapling address into
/// `unified_addresses`, not silently discard it.
#[test]
fn add_sapling_address_below_limit_proceeds_past_guard() {
    let mut wallet = test_wallet();
    let (_, _, addr) = default_zaddr();
    let account_id = zip32::AccountId::try_from(0).unwrap();

    let fvk =
        DiversifiableFullViewingKey::try_from(wallet.unified_key_store.get(&account_id).unwrap())
            .unwrap();
    let (first_valid_diversifier, _) = fvk.find_address(DiversifierIndex::new()).unwrap();

    let result =
        SyncWallet::add_sapling_address(&mut wallet, account_id, addr, first_valid_diversifier);
    assert!(result.is_ok());
    assert!(
        wallet
            .unified_addresses()
            .values()
            .any(|ua| ua.sapling() == Some(&addr)),
        "sapling address must appear in unified_addresses after proceeding past the guard"
    );
}

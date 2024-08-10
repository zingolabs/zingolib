//! functionality for testing the save and load functions of LightWallet.
//! do not compile test-elevation feature for production.

use crate::get_base_address_macro;

use super::LightWallet;

impl LightWallet {
    /// connects a wallet to TestNet server.
    pub async fn unsafe_from_buffer_testnet(data: &[u8]) -> Self {
        let config = crate::config::ZingoConfig::create_testnet();
        Self::read_internal(data, &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet file!: {}", e))
            .unwrap()
    }
}

/// example wallets
/// including from different versions of the software.
pub mod examples;

/// tests
#[cfg(test)]
pub mod tests;

async fn loaded_wallet_assert(wallet: LightWallet, expected_balance: u64, num_addresses: usize) {
    let expected_mnemonic = (
        zcash_primitives::zip339::Mnemonic::from_phrase(
            crate::testvectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        )
        .unwrap(),
        0,
    );
    assert_eq!(wallet.mnemonic(), Some(&expected_mnemonic));

    let expected_wc = crate::wallet::keys::unified::WalletCapability::new_from_phrase(
        &wallet.transaction_context.config,
        &expected_mnemonic.0,
        expected_mnemonic.1,
    )
    .unwrap();
    let wc = wallet.wallet_capability();

    // We don't want the WalletCapability to impl. `Eq` (because it stores secret keys)
    // so we have to compare each component instead

    // Compare Orchard
    let crate::wallet::keys::unified::Capability::Spend(orchard_sk) = &wc.orchard else {
        panic!("Expected Orchard Spending Key");
    };
    assert_eq!(
        orchard_sk.to_bytes(),
        orchard::keys::SpendingKey::try_from(&expected_wc)
            .unwrap()
            .to_bytes()
    );

    // Compare Sapling
    let crate::wallet::keys::unified::Capability::Spend(sapling_sk) = &wc.sapling else {
        panic!("Expected Sapling Spending Key");
    };
    assert_eq!(
        sapling_sk,
        &zcash_client_backend::keys::sapling::ExtendedSpendingKey::try_from(&expected_wc).unwrap()
    );

    // Compare transparent
    let crate::wallet::keys::unified::Capability::Spend(transparent_sk) = &wc.transparent else {
        panic!("Expected transparent extended private key");
    };
    assert_eq!(
        transparent_sk,
        &crate::wallet::keys::extended_transparent::ExtendedPrivKey::try_from(&expected_wc)
            .unwrap()
    );

    assert_eq!(wc.addresses().len(), num_addresses);
    for addr in wc.addresses().iter() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_some());
        assert!(addr.transparent().is_some());
    }

    let client = crate::lightclient::LightClient::create_from_wallet_async(wallet)
        .await
        .unwrap();
    let balance = client.do_balance().await;
    assert_eq!(balance.orchard_balance, Some(expected_balance));
    if expected_balance > 0 {
        let _ = crate::testutils::lightclient::from_inputs::quick_send(
            &client,
            vec![(&get_base_address_macro!(client, "sapling"), 11011, None)],
        )
        .await
        .unwrap();
        let _ = client.do_sync(true).await.unwrap();
        let _ = crate::testutils::lightclient::from_inputs::quick_send(
            &client,
            vec![(
                &crate::get_base_address_macro!(client, "transparent"),
                28000,
                None,
            )],
        )
        .await
        .unwrap();
    }
}

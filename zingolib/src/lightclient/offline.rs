//! Tests for offline (no-indexer) LightClient construction and offline-safe operations.

#[cfg(test)]
mod test {
    use zingo_test_vectors::seeds;

    use crate::{
        config::{ClientConfig, WalletConfig},
        lightclient::{LightClient, error::LightClientError},
        testutils::default_test_wallet_settings,
    };

    async fn create_offline_client() -> LightClient {
        let config = ClientConfig::builder()
            .set_wallet_config(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 419200,
                wallet_settings: default_test_wallet_settings(),
            })
            // no .set_indexer_uri() → offline mode
            .build()
            .unwrap();
        LightClient::new(config, true).await.unwrap()
    }

    #[tokio::test]
    async fn offline_construction_succeeds_without_network() {
        let _lc = create_offline_client().await;
    }

    #[tokio::test]
    async fn offline_indexer_uri_is_none() {
        let lc = create_offline_client().await;
        assert!(lc.indexer_uri().is_none());
    }

    #[tokio::test]
    async fn offline_balance_query_works() {
        let lc = create_offline_client().await;
        // account_balance works offline: it reads cached wallet state.
        // An unsynced wallet succeeds with zero/None balances; it does NOT return Offline.
        let result = lc.account_balance(zip32::AccountId::ZERO).await;
        assert!(
            result.is_ok(),
            "expected Ok from offline balance query, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn offline_address_query_works() {
        let lc = create_offline_client().await;
        let addrs = lc.unified_addresses_json().await;
        assert!(!addrs.is_empty(), "expected at least one address");
    }

    #[tokio::test]
    async fn offline_sync_returns_offline_error() {
        let mut lc = create_offline_client().await;
        assert!(matches!(lc.sync().await, Err(LightClientError::Offline)));
    }

    #[tokio::test]
    async fn offline_rescan_returns_offline_error() {
        let mut lc = create_offline_client().await;
        assert!(matches!(lc.rescan().await, Err(LightClientError::Offline)));
    }

    #[tokio::test]
    async fn offline_send_stored_proposal_returns_offline() {
        let mut lc = create_offline_client().await;
        // Offline always takes priority, even when there is no stored proposal.
        assert!(matches!(
            lc.send_stored_proposal(false).await,
            Err(LightClientError::Offline)
        ));
    }

    #[tokio::test]
    async fn offline_send_stored_proposal_preserves_proposal() {
        let mut lc = create_offline_client().await;

        // Manually store a proposal in the wallet.
        let proposal = crate::mocks::proposal::minimal_transfer_proposal();
        lc.wallet().write().await.store_proposal(proposal);

        // send_stored_proposal while offline must return Offline without consuming the proposal.
        assert!(matches!(
            lc.send_stored_proposal(false).await,
            Err(LightClientError::Offline)
        ));

        // Proposal must still be present. Take it out to verify.
        assert!(
            lc.wallet().write().await.take_proposal().is_some(),
            "proposal must survive an offline send attempt"
        );
    }

    /// Offline signing (ADR 0006): an Indexerless client calculates (selects
    /// witnesses, proves, and signs) a real transaction from the
    /// stored proposal over synthetic wallet state, with no Indexer
    /// anywhere. The signed transaction lands in the wallet with
    /// `Calculated` status with its expiry height retargeted to the top
    /// of the wallet's consensus-branch epoch (the longest expiry the
    /// epoch permits, so the stale offline chain view cannot invalidate
    /// it before transmission. Issue #2455), and only the transmission
    /// half demands a connection.
    #[tokio::test]
    async fn offline_calculate_signs_and_transmit_refuses() {
        use crate::testutils::lightclient::from_inputs;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
        use zingo_status::confirmation_status::ConfirmationStatus;

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        // Proposing stores the proposal in the wallet, as it always has.
        from_inputs::propose(
            &mut client,
            vec![(zingo_test_vectors::EXT_TADDR, 10_000, None)],
        )
        .await
        .unwrap();

        let calculated_txids = client.calculate_stored_proposal().await.unwrap();
        {
            let wallet = client.wallet().read().await;
            for txid in calculated_txids.iter() {
                let transaction = wallet.wallet_transactions.get(txid).unwrap();
                assert!(matches!(
                    transaction.status(),
                    ConfirmationStatus::Calculated(_)
                ));
                // The synthetic wallet's regtest chain activates every
                // upgrade at height 1, so the wallet's epoch is the newest
                // one and no later activation is scheduled: the retarget
                // lifts the target to the highest height whose expiry
                // ZIP 203 can encode, and expiry lands at threshold − 1.
                assert_eq!(
                    u32::from(transaction.transaction().expiry_height()),
                    crate::lightclient::send::ZIP_203_EXPIRY_HEIGHT_THRESHOLD - 1,
                    "an Indexerless calculation must retarget expiry to the \
                     top of the wallet's consensus-branch epoch"
                );
            }
        }

        // Transmission is the half that demands an Indexer; the Calculated
        // transactions stay in the wallet for a later, connected attempt.
        assert!(matches!(
            client.transmit_calculated(calculated_txids).await,
            Err(LightClientError::Offline)
        ));
    }

    #[tokio::test]
    async fn offline_create_account_works() {
        let mut lc = create_offline_client().await;
        lc.create_account().await.unwrap();
    }

    #[tokio::test]
    async fn offline_recovery_info_returns_seed() {
        let lc = create_offline_client().await;
        let info = lc
            .recovery_info()
            .await
            .expect("spending wallet has recovery info");
        assert_eq!(info.seed_phrase, seeds::HOSPITAL_MUSEUM_SEED);
    }

    #[tokio::test]
    async fn offline_clear_proposal_does_not_panic() {
        let mut lc = create_offline_client().await;
        lc.clear_proposal().await; // should be a no-op, not a panic
    }

    /// Round-trip: create wallet → force save → reload from file path (offline).
    #[tokio::test]
    async fn file_pointer_roundtrip() {
        let dir = tempfile::tempdir().unwrap();

        // Create wallet and force-save it.
        let initial_addrs = {
            let config = ClientConfig::builder()
                .set_wallet_dir(dir.path().to_path_buf())
                .set_wallet_config(WalletConfig::MnemonicPhrase {
                    mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                    no_of_accounts: 1.try_into().unwrap(),
                    birthday: 419200,
                    wallet_settings: default_test_wallet_settings(),
                })
                .build()
                .unwrap();
            let mut lc = LightClient::new(config, true).await.unwrap();
            let addrs = lc.unified_addresses_json().await;
            lc.flush().await.unwrap();
            addrs
        };

        // Reload from file path, offline, with no set_indexer_uri().
        let loaded_addrs = {
            let config = ClientConfig::builder()
                .set_wallet_dir(dir.path().to_path_buf())
                .set_wallet_config(WalletConfig::Read)
                .build()
                .unwrap();
            let lc = LightClient::new(config, false).await.unwrap();
            assert!(lc.indexer_uri().is_none(), "reload should be offline");
            assert_eq!(
                lc.chain_type(),
                crate::config::ChainType::Mainnet,
                "chain type must survive round-trip"
            );
            assert_eq!(lc.birthday(), 419200, "birthday must survive round-trip");
            assert_eq!(
                lc.mnemonic_phrase().as_deref(),
                Some(seeds::HOSPITAL_MUSEUM_SEED),
                "mnemonic must survive round-trip"
            );
            lc.unified_addresses_json().await
        };

        assert_eq!(
            initial_addrs, loaded_addrs,
            "addresses must survive save/load round-trip"
        );
    }

    #[tokio::test]
    async fn file_pointer_missing_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        // No wallet file written, so Read should fail with FileError.
        let config = ClientConfig::builder()
            .set_wallet_dir(dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::Read)
            .build()
            .unwrap();
        let result = LightClient::new(config, false).await;
        assert!(
            matches!(result, Err(LightClientError::FileError(_))),
            "expected FileError for missing wallet, got: {result:?}"
        );
    }
}

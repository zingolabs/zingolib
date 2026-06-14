use std::{num::NonZeroU32, time::Duration};

use incrementalmerkletree::Position;
use pepper_sync::sync::ScanPriority;
use pepper_sync::wallet::ShardTrees;
use shardtree::store::ShardStore;
use zcash_local_net::validator::Validator;
use zcash_protocol::consensus::BlockHeight;
use zingo_common_components::protocol::ActivationHeights;
use zingo_netutils::lightwallet_protocol::GetSubtreeRootsArg;
use zingo_netutils::{GrpcIndexer, Indexer};
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::config::{ChainType, ClientConfig, WalletConfig};
use zingolib::data::PollReport;
use zingolib::lightclient::DEFAULT_REQUEST_TIMEOUT;
use zingolib::testutils::default_test_wallet_settings;
use zingolib::testutils::lightclient::from_inputs::quick_send;
use zingolib::testutils::paths::get_cargo_manifest_dir;
use zingolib::testutils::tempfile::TempDir;
use zingolib::{
    config::{DEFAULT_INDEXER_URI, construct_lightwalletd_uri},
    get_base_address_macro,
    lightclient::LightClient,
    testutils::lightclient::from_inputs::{self},
};
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};

#[ignore = "temporary mainnet test for sync development"]
#[tokio::test]
async fn sync_mainnet_test() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Ring to work as a default");
    tracing_subscriber::fmt().init();

    let uri = construct_lightwalletd_uri(Some(DEFAULT_INDEXER_URI.to_string())).unwrap();
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = ClientConfig::builder()
        .set_indexer_uri(uri.clone())
        .set_chain_type(ChainType::Mainnet)
        .set_wallet_dir(temp_path)
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            birthday: 1_500_000,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    let mut lightclient = LightClient::new(config, true).await.unwrap();

    lightclient.sync().await.unwrap();
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        {
            let wallet = lightclient.wallet().read().await;
            tracing::info!(
                "{}",
                json::JsonValue::from(pepper_sync::sync_status(&*wallet).await.unwrap())
            );
            tracing::info!("WALLET DEBUG:");
            tracing::info!("uas: {}", wallet.unified_addresses().len());
            tracing::info!("taddrs: {}", wallet.transparent_addresses().len());
            tracing::info!("blocks: {}", wallet.wallet_blocks.len());
            tracing::info!("txs: {}", wallet.wallet_transactions.len());
            tracing::info!("nullifiers o: {}", wallet.nullifier_map.orchard.len());
            tracing::info!("nullifiers s: {}", wallet.nullifier_map.sapling.len());
            tracing::info!("outpoints: {}", wallet.outpoint_map.len());
        }
        lightclient.wallet().write().await.save().unwrap();
    }

    // let wallet = lightclient.wallet.read().await;
    // dbg!(&wallet.wallet_blocks);
    // dbg!(&wallet.nullifier_map);
    // dbg!(&wallet.sync_state);
}

#[tokio::test]
async fn add_subtree_roots() {
    fn assert_subtree_roots_match_server(
        shard_trees: &mut ShardTrees,
        sapling_subtree_roots_server: Vec<Vec<u8>>,
        orchard_subtree_roots_server: Vec<Vec<u8>>,
    ) {
        let mut sapling_shard_addrs = shard_trees.sapling.store().get_shard_roots().unwrap();
        if sapling_shard_addrs.len() > sapling_subtree_roots_server.len() {
            sapling_shard_addrs.pop();
        }
        assert!(
            sapling_shard_addrs.len() == sapling_subtree_roots_server.len(),
            "no of sapling shard roots wallet: {}, no of sapling shard roots server: {}",
            sapling_shard_addrs.len(),
            sapling_subtree_roots_server.len()
        );
        let mut sapling_subtree_roots_wallet = Vec::new();
        for addr in sapling_shard_addrs {
            let root = shard_trees
                .sapling
                .root(addr, Position::from(u64::MAX))
                .unwrap();
            sapling_subtree_roots_wallet.push(root.to_bytes().to_vec());
        }

        assert!(!sapling_subtree_roots_server.is_empty());
        assert!(
            sapling_subtree_roots_wallet.len() == sapling_subtree_roots_server.len(),
            "no of sapling shard roots wallet: {}, no of sapling shard roots server: {}",
            sapling_subtree_roots_wallet.len(),
            sapling_subtree_roots_server.len()
        );
        sapling_subtree_roots_wallet
            .iter()
            .zip(sapling_subtree_roots_server.clone())
            .for_each(|(wallet_root, server_root)| {
                assert!(wallet_root.as_slice() == server_root.as_slice());
            });

        let mut orchard_shard_addrs = shard_trees.orchard.store().get_shard_roots().unwrap();
        if orchard_shard_addrs.len() > orchard_subtree_roots_server.len() {
            orchard_shard_addrs.pop();
        }
        assert!(
            orchard_shard_addrs.len() == orchard_subtree_roots_server.len(),
            "no of orchard shard roots wallet: {}, no of orchard shard roots server: {}",
            orchard_shard_addrs.len(),
            orchard_subtree_roots_server.len()
        );
        let mut orchard_subtree_roots_wallet = Vec::new();
        for addr in orchard_shard_addrs {
            let root = shard_trees
                .orchard
                .root(addr, Position::from(u64::MAX))
                .unwrap();
            orchard_subtree_roots_wallet.push(root.to_bytes().to_vec());
        }

        assert!(!orchard_subtree_roots_server.is_empty());
        assert!(
            orchard_subtree_roots_wallet.len() == orchard_subtree_roots_server.len(),
            "no of orchard shard roots wallet: {}, no of orchard shard roots server: {}",
            orchard_subtree_roots_wallet.len(),
            orchard_subtree_roots_server.len()
        );
        orchard_subtree_roots_wallet
            .iter()
            .zip(orchard_subtree_roots_server)
            .for_each(|(wallet_root, server_root)| {
                assert!(wallet_root.as_slice() == server_root.as_slice());
            });
    }

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Ring to work as a default");

    let uri = construct_lightwalletd_uri(Some(DEFAULT_INDEXER_URI.to_string())).unwrap();
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = ClientConfig::builder()
        .set_indexer_uri(uri.clone())
        .set_chain_type(ChainType::Mainnet)
        .set_wallet_dir(temp_path)
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            birthday: 2_000_000,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    let mut lightclient = LightClient::new(config, true).await.unwrap();

    let mut grpc_client = GrpcIndexer::new(lightclient.indexer_uri().clone())
        .await
        .unwrap();

    let mut sapling_subtree_roots_server = Vec::new();
    let mut sapling_subtree_roots_stream = grpc_client
        .get_subtree_roots(
            GetSubtreeRootsArg {
                start_index: 0,
                shielded_protocol: 0,
                max_entries: 0,
            },
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
    while let Some(root) = sapling_subtree_roots_stream.message().await.unwrap() {
        sapling_subtree_roots_server.push(root.root_hash);
    }

    let mut orchard_subtree_roots_server = Vec::new();
    let mut orchard_subtree_roots_stream = grpc_client
        .get_subtree_roots(
            GetSubtreeRootsArg {
                start_index: 0,
                shielded_protocol: 1,
                max_entries: 0,
            },
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
    while let Some(root) = orchard_subtree_roots_stream.message().await.unwrap() {
        orchard_subtree_roots_server.push(root.root_hash);
    }

    lightclient.sync().await.unwrap();
    while !(lightclient
        .wallet()
        .read()
        .await
        .sync_state
        .scan_ranges()
        .iter()
        .any(|range| range.priority() == ScanPriority::Scanning)
        || matches!(lightclient.poll_sync(), PollReport::Ready(_)))
    {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = lightclient.stop_sync();
    let _ = lightclient.await_sync().await;

    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;

        assert_subtree_roots_match_server(
            shard_trees,
            sapling_subtree_roots_server.clone(),
            orchard_subtree_roots_server.clone(),
        );

        shard_trees
            .sapling
            .store_mut()
            .truncate_shards(500)
            .unwrap();
        let sapling_shard_addrs = shard_trees.sapling.store().get_shard_roots().unwrap();
        assert!(sapling_shard_addrs.len() != sapling_subtree_roots_server.len());

        shard_trees
            .orchard
            .store_mut()
            .truncate_shards(500)
            .unwrap();
        let orchard_shard_addrs = shard_trees.orchard.store().get_shard_roots().unwrap();
        assert!(orchard_shard_addrs.len() != orchard_subtree_roots_server.len());
    }

    lightclient.sync().await.unwrap();
    while !(lightclient
        .wallet()
        .read()
        .await
        .sync_state
        .scan_ranges()
        .iter()
        .any(|range| range.priority() == ScanPriority::Scanning)
        || matches!(lightclient.poll_sync(), PollReport::Ready(_)))
    {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = lightclient.stop_sync();
    let _ = lightclient.await_sync().await;

    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;

        assert_subtree_roots_match_server(
            shard_trees,
            sapling_subtree_roots_server.clone(),
            orchard_subtree_roots_server.clone(),
        );
    }
}

// temporary test for sync development
#[ignore = "sync development only"]
#[allow(unused_mut, unused_variables)]
#[tokio::test]
async fn sync_test() {
    tracing_subscriber::fmt().init();

    let (_local_net, mut faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(5_000_000).await;

    // let recipient_ua = get_base_address_macro!(&recipient, "unified");
    let recipient_taddr = get_base_address_macro!(&recipient, "transparent");
    from_inputs::quick_send(&mut faucet, vec![(&recipient_taddr, 100_000, None)])
        .await
        .unwrap();

    recipient.sync_and_await().await.unwrap();

    // increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();

    // tracing::info!("{}", recipient.transaction_summaries().await.unwrap());
    tracing::info!("{}", recipient.value_transfers(false).await.unwrap());
    tracing::info!(
        "{}",
        recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
    );
    tracing::info!(
        "{:?}",
        recipient.propose_shield(zip32::AccountId::ZERO).await
    );

    // tracing::info!(
    //     "{:?}",
    //     recipient
    //         .get_spendable_shielded_balance(
    //             zcash_address::ZcashAddress::try_from_encoded(&recipient_ua).unwrap(),
    //             false
    //         )
    //         .await
    //         .unwrap()
    // );
    // let wallet = recipient.wallet.lock().await;
    // dbg!(wallet.wallet_blocks.len());
}

#[ignore = "only for building chain cache"]
#[tokio::test]
async fn store_all_checkpoints_in_verification_window_chain_cache() {
    let (mut local_net, mut faucet, recipient) = scenarios::faucet_recipient_default().await;

    let recipient_orchard_addr = get_base_address_macro!(recipient, "unified");
    let recipient_sapling_addr = get_base_address_macro!(recipient, "sapling");

    for _ in 0..27 {
        quick_send(&mut faucet, vec![(&recipient_orchard_addr, 10_000, None)])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();

        quick_send(&mut faucet, vec![(&recipient_sapling_addr, 10_000, None)])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();

        quick_send(&mut faucet, vec![(&recipient_orchard_addr, 10_000, None)])
            .await
            .unwrap();
        quick_send(&mut faucet, vec![(&recipient_sapling_addr, 10_000, None)])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 2)
            .await
            .unwrap();
    }

    local_net
        .validator_mut()
        .cache_chain(get_cargo_manifest_dir().join("store_all_checkpoints_test"))
        .await;
}

#[ignore = "ignored until we add framework for chain caches as we don't want to check these into the zingolib repo"]
#[tokio::test]
async fn store_all_checkpoints_in_verification_window() {
    let (_local_net, lightclient) = scenarios::unfunded_client(
        ActivationHeights::default(),
        Some(get_cargo_manifest_dir().join("store_all_checkpoints_test")),
    )
    .await;

    for height in 12..112 {
        assert!(
            lightclient
                .wallet()
                .read()
                .await
                .shard_trees
                .sapling
                .store()
                .get_checkpoint(&BlockHeight::from_u32(height))
                .unwrap()
                .is_some(),
            "missing sapling checkpoint at height {height}"
        );
        assert!(
            lightclient
                .wallet()
                .read()
                .await
                .shard_trees
                .orchard
                .store()
                .get_checkpoint(&BlockHeight::from_u32(height))
                .unwrap()
                .is_some(),
            "missing orchard checkpoint at height {height}"
        );
    }
}

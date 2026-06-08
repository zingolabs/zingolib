use std::{num::NonZeroU32, time::Duration};

use bip0039::Mnemonic;
use incrementalmerkletree::Position;
use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use pepper_sync::sync::ScanPriority;
use pepper_sync::wallet::ShardTrees;
use shardtree::store::ShardStore;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient;
use zcash_local_net::validator::Validator;
use zcash_protocol::consensus::BlockHeight;
use zingo_common_components::protocol::activation_heights::for_test::all_height_one_nus;
use zingo_netutils::GetClientError;
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::data::PollReport;
use zingolib::testutils::lightclient::from_inputs::quick_send;
use zingolib::testutils::paths::get_cargo_manifest_dir;
use zingolib::testutils::tempfile::TempDir;
use zingolib::{
    config::{DEFAULT_LIGHTWALLETD_SERVER, construct_lightwalletd_uri, load_clientconfig},
    get_base_address_macro,
    lightclient::LightClient,
    testutils::lightclient::from_inputs::{self},
    wallet::{LightWallet, WalletBase, WalletSettings},
};
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};

#[ignore = "temporary mainnet test for sync development"]
#[tokio::test]
async fn sync_mainnet_test() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Ring to work as a default");
    tracing_subscriber::fmt().init();

    let uri = construct_lightwalletd_uri(Some(DEFAULT_LIGHTWALLETD_SERVER.to_string()));
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = load_clientconfig(
        uri.clone(),
        Some(temp_path),
        zingolib::config::ChainType::Mainnet,
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: PerformanceLevel::High,
            },
            min_confirmations: NonZeroU32::try_from(1).unwrap(),
        },
        1.try_into().unwrap(),
        "".to_string(),
    )
    .unwrap();
    let mut lightclient = LightClient::create_from_wallet(
        LightWallet::new(
            config.chain,
            WalletBase::Mnemonic {
                mnemonic: Mnemonic::from_phrase(HOSPITAL_MUSEUM_SEED.to_string()).unwrap(),
                no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            },
            1_500_000.into(),
            config.wallet_settings.clone(),
        )
        .unwrap(),
        config,
        true,
    )
    .unwrap();

    lightclient.sync().await.unwrap();
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        {
            let wallet = lightclient.wallet.read().await;
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
        lightclient.wallet.write().await.save().unwrap();
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

    // temporary client creation until zingolib v5 lands with new netutils update (imminent!)
    async fn get_client(
        uri: http::Uri,
    ) -> Result<CompactTxStreamerClient<Channel>, GetClientError> {
        let scheme = uri.scheme_str().ok_or(GetClientError::InvalidScheme)?;
        if scheme != "http" && scheme != "https" {
            return Err(GetClientError::InvalidScheme);
        }
        let _authority = uri.authority().ok_or(GetClientError::InvalidAuthority)?;

        let endpoint = Endpoint::from_shared(uri.to_string())?.tcp_nodelay(true);

        let channel = if scheme == "https" {
            let tls = ClientTlsConfig::new().with_webpki_roots();
            endpoint.tls_config(tls)?.connect().await?
        } else {
            endpoint.connect().await?
        };

        Ok(CompactTxStreamerClient::new(channel))
    }
    #[derive(Debug, thiserror::Error)]
    pub enum GetClientError {
        #[error("bad uri: invalid scheme")]
        InvalidScheme,

        #[error("bad uri: invalid authority")]
        InvalidAuthority,

        #[error("bad uri: invalid path and/or query")]
        InvalidPathAndQuery,

        #[error(transparent)]
        Transport(#[from] tonic::transport::Error),
    }

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Ring to work as a default");

    let uri = construct_lightwalletd_uri(Some(DEFAULT_LIGHTWALLETD_SERVER.to_string()));
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = load_clientconfig(
        uri.clone(),
        Some(temp_path),
        zingolib::config::ChainType::Mainnet,
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::disabled(),
                performance_level: PerformanceLevel::High,
            },
            min_confirmations: NonZeroU32::try_from(1).unwrap(),
        },
        1.try_into().unwrap(),
        "".to_string(),
    )
    .unwrap();
    let mut lightclient = LightClient::create_from_wallet(
        LightWallet::new(
            config.chain,
            WalletBase::Mnemonic {
                mnemonic: Mnemonic::from_phrase(HOSPITAL_MUSEUM_SEED.to_string()).unwrap(),
                no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            },
            2_000_000.into(),
            config.wallet_settings.clone(),
        )
        .unwrap(),
        config,
        true,
    )
    .unwrap();

    let mut grpc_client = get_client(lightclient.config.get_lightwalletd_uri())
        .await
        .unwrap();

    let request = tonic::Request::new(zcash_client_backend::proto::service::GetSubtreeRootsArg {
        start_index: 0,
        shielded_protocol: 0,
        max_entries: 0,
    });
    let mut sapling_subtree_roots_server = Vec::new();
    let mut sapling_subtree_roots_stream = grpc_client
        .get_subtree_roots(request)
        .await
        .unwrap()
        .into_inner();
    while let Some(root) = sapling_subtree_roots_stream.message().await.unwrap() {
        sapling_subtree_roots_server.push(root.root_hash);
    }

    let request = tonic::Request::new(zcash_client_backend::proto::service::GetSubtreeRootsArg {
        start_index: 0,
        shielded_protocol: 1,
        max_entries: 0,
    });
    let mut orchard_subtree_roots_server = Vec::new();
    let mut orchard_subtree_roots_stream = grpc_client
        .get_subtree_roots(request)
        .await
        .unwrap()
        .into_inner();
    while let Some(root) = orchard_subtree_roots_stream.message().await.unwrap() {
        orchard_subtree_roots_server.push(root.root_hash);
    }

    lightclient.sync().await.unwrap();
    while !(lightclient
        .wallet
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
        let shard_trees = &mut lightclient.wallet.write().await.shard_trees;

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
        .wallet
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
        let shard_trees = &mut lightclient.wallet.write().await.shard_trees;

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
        .cache_chain(get_cargo_manifest_dir().join("store_all_checkpoints_test"));
}

#[ignore = "ignored until we add framework for chain caches as we don't want to check these into the zingolib repo"]
#[tokio::test]
async fn store_all_checkpoints_in_verification_window() {
    let (_local_net, lightclient) = scenarios::unfunded_client(
        all_height_one_nus(),
        Some(get_cargo_manifest_dir().join("store_all_checkpoints_test")),
    )
    .await;

    for height in 12..112 {
        assert!(
            lightclient
                .wallet
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
                .wallet
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

use darkside_tests::{
    constants::{
        ADVANCED_REORG_TESTS_USER_WALLET, BRANCH_ID, REORG_CHANGES_INCOMING_TX_HEIGHT_AFTER,REORG_CHANGES_INCOMING_TX_INDEX_BEFORE, REORG_CHANGES_INCOMING_TX_INDEX_AFTER,
        REORG_CHANGES_INCOMING_TX_HEIGHT_BEFORE, TREE_STATE_FOLDER_PATH,
    },
    darkside_types::{Empty, TreeState},
    utils::{read_block_dataset, read_lines, DarksideConnector, DarksideHandler},
};

use tokio::time::sleep;
use zcash_primitives::consensus::BlockHeight;
use zingo_testutils::{regtest::get_cargo_manifest_dir, scenarios::setup::ClientBuilder};
use zingoconfig::RegtestNetwork;
use zingolib::lightclient::PoolBalances;

#[tokio::test]
async fn reorg_changes_incoming_tx_height() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));

    prepare_before_tx_height_change_reorg(server_id.clone())
        .await
        .unwrap();

    let light_client = ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone())
        .build_client(
            ADVANCED_REORG_TESTS_USER_WALLET.to_string(),
            202,
            true,
            RegtestNetwork::all_upgrades_active(),
        )
        .await;

    light_client.do_sync(true).await.unwrap();
    assert_eq!(
        light_client.do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(100000000),
            verified_orchard_balance: Some(100000000),
            spendable_orchard_balance: Some(100000000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );

    let before_reorg_transactions = light_client.do_list_txsummaries().await;

    assert_eq!(before_reorg_transactions.len(), 1);
    assert_eq!(
        before_reorg_transactions[0].block_height,
        BlockHeight::from_u32(203)
    );

    prepare_after_tx_height_change_reorg(server_id.clone())
        .await
        .unwrap();

    let reorg_sync_result = light_client.do_sync(true).await;

    match reorg_sync_result {
        Ok(value) => println!("{}", value),
        Err(err_str) => println!("{}", err_str),
    };

    // Assert that balance holds
    assert_eq!(
        light_client.do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(100000000),
            verified_orchard_balance: Some(100000000),
            spendable_orchard_balance: Some(100000000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );

    let after_reorg_transactions = light_client.do_list_txsummaries().await;

    assert_eq!(after_reorg_transactions.len(), 1);
    assert_eq!(
        after_reorg_transactions[0].block_height,
        BlockHeight::from_u32(206)
    );
}

async fn prepare_before_tx_height_change_reorg(uri: http::Uri) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri.clone());

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    // reset with parameters
    connector
        .reset(202, String::from(BRANCH_ID), String::from("regtest"))
        .await
        .unwrap();

    let dataset_path = format!(
        "{}/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        REORG_CHANGES_INCOMING_TX_HEIGHT_BEFORE
    );

    println!("dataset path: {}", dataset_path);

    connector
        .stage_blocks_stream(read_block_dataset(dataset_path))
        .await?;

    for i in 201..207 {
        let tree_state_path = format!(
            "{}/{}/{}.json",
            get_cargo_manifest_dir().to_string_lossy(),
            TREE_STATE_FOLDER_PATH,
            i
        );
        let tree_state = TreeState::from_file(tree_state_path).unwrap();
        connector.add_tree_state(tree_state).await.unwrap();
    }

    connector.apply_staged(204).await?;

    sleep(std::time::Duration::new(1, 0)).await;

    Ok(())
}

async fn prepare_after_tx_height_change_reorg(uri: http::Uri) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri.clone());

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    let dataset_path = format!(
        "{}/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        REORG_CHANGES_INCOMING_TX_HEIGHT_AFTER
    );
    connector
        .stage_blocks_stream(
            read_lines(dataset_path)
                .unwrap()
                .map(|line| line.unwrap())
                .collect(),
        )
        .await?;

    connector.apply_staged(206).await?;

    sleep(std::time::Duration::new(1, 0)).await;

    Ok(())
}

#[tokio::test]
async fn reorg_changes_incoming_tx_index() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));

    prepare_before_tx_index_change_reorg(server_id.clone())
        .await
        .unwrap();

    let light_client = ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone())
        .build_client(
            ADVANCED_REORG_TESTS_USER_WALLET.to_string(),
            202,
            true,
            RegtestNetwork::all_upgrades_active(),
        )
        .await;

    light_client.do_sync(true).await.unwrap();
    assert_eq!(
        light_client.do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(100000000),
            verified_orchard_balance: Some(100000000),
            spendable_orchard_balance: Some(100000000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );

    let before_reorg_transactions = light_client.do_list_txsummaries().await;

    assert_eq!(before_reorg_transactions.len(), 1);
    assert_eq!(
        before_reorg_transactions[0].block_height,
        BlockHeight::from_u32(203)
    );

    prepare_after_tx_index_change_reorg(server_id.clone())
        .await
        .unwrap();

    let reorg_sync_result = light_client.do_sync(true).await;

    match reorg_sync_result {
        Ok(value) => println!("{}", value),
        Err(err_str) => println!("{}", err_str),
    };

    // Assert that balance holds
    assert_eq!(
        light_client.do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(100000000),
            verified_orchard_balance: Some(100000000),
            spendable_orchard_balance: Some(100000000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );

    let after_reorg_transactions = light_client.do_list_txsummaries().await;

    assert_eq!(after_reorg_transactions.len(), 1);
    assert_eq!(
        after_reorg_transactions[0].block_height,
        BlockHeight::from_u32(203)
    );
}

async fn prepare_before_tx_index_change_reorg(uri: http::Uri) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri.clone());

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    // reset with parameters
    connector
        .reset(202, String::from(BRANCH_ID), String::from("regtest"))
        .await
        .unwrap();

    let dataset_path = format!(
        "{}/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        REORG_CHANGES_INCOMING_TX_INDEX_BEFORE
    );

    println!("dataset path: {}", dataset_path);

    connector
        .stage_blocks_stream(read_block_dataset(dataset_path))
        .await?;

    for i in 201..207 {
        let tree_state_path = format!(
            "{}/{}/{}.json",
            get_cargo_manifest_dir().to_string_lossy(),
            TREE_STATE_FOLDER_PATH,
            i
        );
        let tree_state = TreeState::from_file(tree_state_path).unwrap();
        connector.add_tree_state(tree_state).await.unwrap();
    }

    connector.apply_staged(204).await?;

    sleep(std::time::Duration::new(1, 0)).await;

    Ok(())
}

async fn prepare_after_tx_index_change_reorg(uri: http::Uri) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri.clone());

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    let dataset_path = format!(
        "{}/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        REORG_CHANGES_INCOMING_TX_INDEX_AFTER
    );
    connector
        .stage_blocks_stream(
            read_lines(dataset_path)
                .unwrap()
                .map(|line| line.unwrap())
                .collect(),
        )
        .await?;

    connector.apply_staged(206).await?;

    sleep(std::time::Duration::new(1, 0)).await;

    Ok(())
}
#[tokio::test]
async fn test_read_block_dataset() {
    let dataset_path = format!(
        "{}/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        REORG_CHANGES_INCOMING_TX_HEIGHT_BEFORE
    );
    let blocks = read_block_dataset(dataset_path);
    assert_eq!(blocks.len(), 21)
}

#[tokio::test]
async fn test_read_tree_state_from_file() {
    let tree_state_path = format!(
        "{}/{}/{}.json",
        get_cargo_manifest_dir().to_string_lossy(),
        TREE_STATE_FOLDER_PATH,
        203
    );

    println!("{}", tree_state_path);

    let tree_state = TreeState::from_file(tree_state_path).unwrap();

    assert_eq!(tree_state.network.as_str(), "regtest");
    assert_eq!(tree_state.height, 203);
    assert_eq!(
        tree_state.hash,
        "016da97020ab191559f34f1d3f992ce2ec7c609cb0e5b932c45f1693eeb2192f"
    );
    assert_eq!(tree_state.time, 1694454196);
    assert_eq!(tree_state.sapling_tree, "000000");
    assert_eq!(tree_state.orchard_tree, "01136febe0db97210efb679e378d3b3a49d6ac72d0161ae478b1faaa9bd26a2118012246dd85ba2d9510caa03c40f0b75f7b02cb0cfac88ec1c4b9193d58bb6d44201f000001f0328e13a28669f9a5bd2a1c5301549ea28ccb7237347b9c76c05276952ad135016be8aefe4f98825b5539a2b47b90a8057e52c1e1badc725d67c06b4cc2a32e24000000000000000000000000000000000000000000000000000000");
}

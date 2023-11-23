use darkside_tests::{
    constants::{
        ADVANCED_REORG_TESTS_USER_WALLET, BRANCH_ID, REORG_CHANGES_INCOMING_TX_HEIGHT_AFTER,
        REORG_CHANGES_INCOMING_TX_HEIGHT_BEFORE, REORG_CHANGES_INCOMING_TX_INDEX_AFTER,
        REORG_CHANGES_INCOMING_TX_INDEX_BEFORE, REORG_EXPIRES_INCOMING_TX_HEIGHT_AFTER,
        REORG_EXPIRES_INCOMING_TX_HEIGHT_BEFORE, TREE_STATE_FOLDER_PATH,
    },
    darkside_types::{Empty, TreeState},
    utils::{read_block_dataset, read_lines, DarksideConnector, DarksideHandler},
};

use tokio::time::sleep;
use zcash_primitives::consensus::BlockHeight;
use zingo_testutils::{regtest::get_cargo_manifest_dir, scenarios::setup::ClientBuilder};
use zingoconfig::RegtestNetwork;
use zingolib::lightclient::PoolBalances;
use zingolib::wallet::data::summaries::ValueTransferKind;
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
async fn reorg_expires_incoming_tx() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));

    prepare_expires_incoming_tx_before_reorg(server_id.clone())
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

    prepare_expires_incoming_tx_after_reorg(server_id.clone())
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
            orchard_balance: Some(0),
            verified_orchard_balance: Some(0),
            spendable_orchard_balance: Some(0),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );

    let after_reorg_transactions = light_client.do_list_txsummaries().await;

    assert_eq!(after_reorg_transactions.len(), 0);
}

async fn prepare_expires_incoming_tx_before_reorg(uri: http::Uri) -> Result<(), String> {
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
        REORG_EXPIRES_INCOMING_TX_HEIGHT_BEFORE
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

async fn prepare_expires_incoming_tx_after_reorg(uri: http::Uri) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri.clone());

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    let dataset_path = format!(
        "{}/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        REORG_EXPIRES_INCOMING_TX_HEIGHT_AFTER
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

// OUTGOING TX TESTS

#[tokio::test]
/// A Re Org occurs and changes the height of an outbound transaction
///
/// Pre-condition: Wallet has funds
///
/// Steps:
/// 1. create fake chain
///    * 1a. sync to latest height
/// 2. send transaction to recipient address
/// 3. getIncomingTransaction
/// 4. stage transaction at `sentTxHeight`
/// 5. applyHeight(sentTxHeight)
/// 6. sync to latest height
///    * 6a. verify that there's a pending transaction with a mined height of sentTxHeight
/// 7. stage 15  blocks from `sentTxHeight`
/// 7. a stage sent tx to `sentTxHeight + 2`
/// 8. `applyHeight(sentTxHeight + 1)` to cause a 1 block reorg
/// 9. sync to latest height
/// 10. verify that there's a pending transaction with -1 mined height
/// 11. `applyHeight(sentTxHeight + 2)`
///    * 11a. sync to latest height
/// 12. verify that there's a pending transaction with a mined height of `sentTxHeight + 2`
/// 13. apply height(`sentTxHeight + 15`)
/// 14. sync to latest height
/// 15. verify that there's no pending transaction and that the tx is displayed on the sentTransactions collection
async fn reorg_changes_outgoing_tx_height() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));

    prepare_changes_outgoing_tx_height_before_reorg(server_id.clone())
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

    let connector = DarksideConnector(server_id.clone());

    let recipient_string = "uregtest1z8s5szuww2cnze042e0re2ez8l3d04zvkp7kslxwdha6tp644srd4nh0xlp8a05avzduc6uavqkxv79x53c60hrc0qsgeza3age2g3qualullukd4s0lsn6mtfup4z8jz6xdz2c05zakhafc7pmw0dwugwu9ljevzgyc3mfwxg9slr87k8l7cq075gl3fgxpr85uuvxhxydrskp2303";

    // Send 100000 zatoshi to some address
    let amount: u64 = 100000;
    let sent_tx_id = light_client
        .do_send([(recipient_string, amount, None)].to_vec())
        .await
        .unwrap();

    println!("SENT TX ID: {:?}", sent_tx_id);

    let mut incoming_transaction_stream = connector.get_incoming_transactions().await.unwrap();
    let tx = incoming_transaction_stream
        .message()
        .await
        .unwrap()
        .unwrap();

    let sent_tx_height: i32 = 205;
    _ = connector.apply_staged(sent_tx_height).await;

    light_client.do_sync(true).await.unwrap();

    let expected_after_send_balance = PoolBalances {
        sapling_balance: Some(0),
        verified_sapling_balance: Some(0),
        spendable_sapling_balance: Some(0),
        unverified_sapling_balance: Some(0),
        orchard_balance: Some(99890000),
        verified_orchard_balance: Some(0),
        spendable_orchard_balance: Some(0),
        unverified_orchard_balance: Some(99890000),
        transparent_balance: Some(0),
    };

    assert_eq!(light_client.do_balance().await, expected_after_send_balance);

    // check that the outgoing transaction has the correct height before
    // the reorg is triggered

    println!("{:?}", light_client.do_list_txsummaries().await);

    assert_eq!(
        light_client
            .do_list_txsummaries()
            .await
            .into_iter()
            .find_map(|v| match v.kind {
                ValueTransferKind::Sent { to_address, amount } => {
                    if to_address.to_string() == recipient_string && amount == 100000 {
                        Some(v.block_height)
                    } else {
                        None
                    }
                }
                _ => {
                    None
                }
            }),
        Some(BlockHeight::from(sent_tx_height as u32))
    );

    //
    // Create reorg
    //

    // stage empty blocks from height 205 to cause a Reorg
    _ = connector.stage_blocks_create(sent_tx_height, 20, 1).await;

    _ = connector
        .stage_transactions_stream([(tx.clone().data, 210)].to_vec())
        .await;

    _ = connector.apply_staged(211).await;

    let reorg_sync_result = light_client.do_sync(true).await;

    match reorg_sync_result {
        Ok(value) => println!("{}", value),
        Err(err_str) => println!("{}", err_str),
    };

    let expected_after_reorg_balance = PoolBalances {
        sapling_balance: Some(0),
        verified_sapling_balance: Some(0),
        spendable_sapling_balance: Some(0),
        unverified_sapling_balance: Some(0),
        orchard_balance: Some(99890000),
        verified_orchard_balance: Some(99890000),
        spendable_orchard_balance: Some(99890000),
        unverified_orchard_balance: Some(0),
        transparent_balance: Some(0),
    };

    // Assert that balance holds
    assert_eq!(
        light_client.do_balance().await,
        expected_after_reorg_balance
    );

    let after_reorg_transactions = light_client.do_list_txsummaries().await;

    assert_eq!(after_reorg_transactions.len(), 3);

    println!("{:?}", light_client.do_list_txsummaries().await);

    // FIXME: This test is broken because if this issue
    // https://github.com/zingolabs/zingolib/issues/622
    // verify that the reorged transaction is in the new height
    // assert_eq!(
    //     light_client
    //         .do_list_txsummaries()
    //         .await
    //         .into_iter()
    //         .find_map(|v| match v.kind {
    //             ValueTransferKind::Sent { to_address, amount } => {
    //                 if to_address.to_string() == recipient_string && amount == 100000 {
    //                     Some(v.block_height)
    //                 } else {
    //                     None
    //                 }
    //             }
    //             _ => {
    //                 None
    //             }
    //         }),
    //     Some(BlockHeight::from(211))
    // );
}

async fn prepare_changes_outgoing_tx_height_before_reorg(uri: http::Uri) -> Result<(), String> {
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

    // this dataset works for this test since we only need funds to send a transaction.
    let dataset_path = format!(
        "{}/{}",
        get_cargo_manifest_dir().to_string_lossy(),
        REORG_EXPIRES_INCOMING_TX_HEIGHT_BEFORE
    );

    println!("dataset path: {}", dataset_path);

    connector
        .stage_blocks_stream(read_block_dataset(dataset_path))
        .await?;

    for i in 201..211 {
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

// UTILS TESTS
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

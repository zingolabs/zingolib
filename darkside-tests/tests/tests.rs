<<<<<<< HEAD:zingocli/tests/darkside/tests.rs
use super::darkside_types::{Empty, RawTransaction, TreeState};
use crate::darkside::{
    constants::{self, BRANCH_ID, DARKSIDE_SEED},
    darkside_connector::DarksideConnector,
    utils::{update_tree_states_for_transaction, DarksideHandler},
};

||||||| dfb77458:zingocli/tests/darkside/tests.rs
use super::darkside_types::{
    darkside_streamer_client::DarksideStreamerClient, DarksideBlock, DarksideBlocksUrl,
    DarksideEmptyBlocks, DarksideHeight, DarksideMetaState, Empty, RawTransaction, TreeState,
};
use crate::darkside::{
    constants::{self, BRANCH_ID, DARKSIDE_SEED},
    utils::{update_tree_states_for_transaction, DarksideHandler},
};

=======
use darkside_tests::{
    constants::DARKSIDE_SEED,
    utils::{
        prepare_darksidewalletd, update_tree_states_for_transaction, DarksideConnector,
        DarksideHandler,
    },
};
>>>>>>> labs/dev:darkside-tests/tests/tests.rs
use tokio::time::sleep;
use zingo_testutils::{data::seeds, scenarios::setup::ClientBuilder};
use zingoconfig::RegtestNetwork;
use zingolib::{get_base_address, lightclient::PoolBalances};

<<<<<<< HEAD:zingocli/tests/darkside/tests.rs
async fn prepare_darksidewalletd(
    uri: http::Uri,
    include_startup_funds: bool,
) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri.clone());

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    // reset with parameters
    connector
        .reset(1, String::from(BRANCH_ID), String::from("regtest"))
        .await
        .unwrap();

    connector
        .stage_blocks_stream(vec![String::from(
            crate::darkside::constants::GENESIS_BLOCK,
        )])
        .await?;

    connector.stage_blocks_create(2, 2, 0).await.unwrap();

    connector
        .add_tree_state(constants::first_tree_state())
        .await
        .unwrap();
    if include_startup_funds {
        connector
            .stage_transactions_stream(vec![(
                hex::decode(constants::TRANSACTION_INCOMING_100TAZ).unwrap(),
                2,
            )])
            .await
            .unwrap();
        let tree_height_2 = update_tree_states_for_transaction(
            &uri,
            RawTransaction {
                data: hex::decode(constants::TRANSACTION_INCOMING_100TAZ).unwrap(),
                height: 2,
            },
            2,
        )
        .await;
        connector
            .add_tree_state(TreeState {
                height: 3,
                ..tree_height_2
            })
            .await
            .unwrap();
    } else {
        for height in [2, 3] {
            connector
                .add_tree_state(TreeState {
                    height,
                    ..constants::first_tree_state()
                })
                .await
                .unwrap();
        }
    }

    sleep(std::time::Duration::new(2, 0)).await;

    connector.apply_staged(3).await?;

    Ok(())
}

||||||| dfb77458:zingocli/tests/darkside/tests.rs
use std::sync::Arc;

use http_body::combinators::UnsyncBoxBody;
use hyper::{client::HttpConnector, Uri};
use tonic::Status;
use tower::{util::BoxCloneService, ServiceExt};

type UnderlyingService = BoxCloneService<
    http::Request<UnsyncBoxBody<prost::bytes::Bytes, Status>>,
    http::Response<hyper::Body>,
    hyper::Error,
>;

macro_rules! define_darkside_connector_methods(
    ($($name:ident (&$self:ident $(,$param:ident: $param_type:ty)*$(,)?) -> $return:ty {$param_packing:expr}),*) => {$(
        #[allow(unused)]
        pub(crate) async fn $name(&$self, $($param: $param_type),*) -> ::std::result::Result<$return, String> {
            let request = ::tonic::Request::new($param_packing);

            let mut client = $self.get_client().await.map_err(|e| format!("{e}"))?;

        client
            .$name(request)
            .await
            .map_err(|e| format!("{}", e))
            .map(|resp| resp.into_inner())
    })*}
);

#[derive(Clone)]
pub struct DarksideConnector(http::Uri);

impl DarksideConnector {
    pub fn new(uri: http::Uri) -> Self {
        Self(uri)
    }

    pub(crate) fn get_client(
        &self,
    ) -> impl std::future::Future<
        Output = Result<DarksideStreamerClient<UnderlyingService>, Box<dyn std::error::Error>>,
    > {
        let uri = Arc::new(self.0.clone());
        async move {
            let mut http_connector = HttpConnector::new();
            http_connector.enforce_http(false);
            let connector = tower::ServiceBuilder::new().service(http_connector);
            let client = Box::new(hyper::Client::builder().http2_only(true).build(connector));
            let uri = uri.clone();
            let svc = tower::ServiceBuilder::new()
                //Here, we take all the pieces of our uri, and add in the path from the Requests's uri
                .map_request(move |mut req: http::Request<tonic::body::BoxBody>| {
                    let uri = Uri::builder()
                        .scheme(uri.scheme().unwrap().clone())
                        .authority(uri.authority().unwrap().clone())
                        //here. The Request's uri contains the path to the GRPC sever and
                        //the method being called
                        .path_and_query(req.uri().path_and_query().unwrap().clone())
                        .build()
                        .unwrap();

                    *req.uri_mut() = uri;
                    req
                })
                .service(client);

            Ok(DarksideStreamerClient::new(svc.boxed_clone()))
        }
    }

    define_darkside_connector_methods!(
        apply_staged(&self, height: i32) -> Empty { DarksideHeight { height } },
        add_tree_state(&self, tree_state: TreeState) -> Empty { tree_state },
        reset(
            &self,
            sapling_activation: i32,
            branch_id: String,
            chain_name: String,
        ) -> Empty {
            DarksideMetaState {
                sapling_activation,
                branch_id,
                chain_name,
            }
        },
        stage_blocks(&self, url: String) -> Empty { DarksideBlocksUrl { url } },
        stage_blocks_create(
            &self,
            height: i32,
            count: i32,
            nonce: i32
        ) -> Empty {
            DarksideEmptyBlocks {
                height,
                count,
                nonce
            }
        },

        stage_blocks_stream(&self, blocks: Vec<String>) -> Empty {
            ::futures_util::stream::iter(
                blocks.into_iter().map(|block| DarksideBlock { block })
            )
        },
        stage_transactions_stream(&self, transactions: Vec<(Vec<u8>, u64)>) -> Empty {
            ::futures_util::stream::iter(
                transactions.into_iter().map(|transaction| {
                    RawTransaction {
                        data: transaction.0,
                        height: transaction.1
                    }
                })
            )
        },
        get_incoming_transactions(&self) -> ::tonic::Streaming<RawTransaction> {
            Empty {}
        }
    );
}

async fn prepare_darksidewalletd(
    uri: http::Uri,
    include_startup_funds: bool,
) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri.clone());

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    // reset with parameters
    connector
        .reset(1, String::from(BRANCH_ID), String::from("regtest"))
        .await
        .unwrap();

    connector
        .stage_blocks_stream(vec![String::from(
            crate::darkside::constants::GENESIS_BLOCK,
        )])
        .await?;

    connector.stage_blocks_create(2, 2, 0).await.unwrap();

    connector
        .add_tree_state(constants::first_tree_state())
        .await
        .unwrap();
    if include_startup_funds {
        connector
            .stage_transactions_stream(vec![(
                hex::decode(constants::TRANSACTION_INCOMING_100TAZ).unwrap(),
                2,
            )])
            .await
            .unwrap();
        let tree_height_2 = update_tree_states_for_transaction(
            &uri,
            RawTransaction {
                data: hex::decode(constants::TRANSACTION_INCOMING_100TAZ).unwrap(),
                height: 2,
            },
            2,
        )
        .await;
        connector
            .add_tree_state(TreeState {
                height: 3,
                ..tree_height_2
            })
            .await
            .unwrap();
    } else {
        for height in [2, 3] {
            connector
                .add_tree_state(TreeState {
                    height,
                    ..constants::first_tree_state()
                })
                .await
                .unwrap();
        }
    }

    sleep(std::time::Duration::new(2, 0)).await;

    connector.apply_staged(3).await?;

    Ok(())
}

=======
>>>>>>> labs/dev:darkside-tests/tests/tests.rs
#[tokio::test]
async fn simple_sync() {
    let darkside_handler = DarksideHandler::default();

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = ClientBuilder::new(server_id, darkside_handler.darkside_dir.clone())
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;

    let result = light_client.do_sync(true).await.unwrap();

    println!("{}", result);

    assert_eq!(result.success, true);
    assert_eq!(result.latest_block, 3);
    assert_eq!(result.total_blocks_synced, 3);
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
}

#[tokio::test]
async fn reorg_away_receipt() {
    let darkside_handler = DarksideHandler::default();

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();

    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone())
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
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
    prepare_darksidewalletd(server_id.clone(), false)
        .await
        .unwrap();
    light_client.do_sync(true).await.unwrap();
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
}

#[tokio::test]
async fn sent_transaction_reorged_into_mempool() {
    let darkside_handler = DarksideHandler::default();

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();

    let mut client_manager =
        ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone());
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = client_manager
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;
    let recipient = client_manager
        .build_client(
            seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            1,
            true,
            regtest_network,
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
    let txid = light_client
        .do_send(vec![(
            &get_base_address!(recipient, "unified"),
            10_000,
            None,
        )])
        .await
        .unwrap();
    println!("{}", txid);
    recipient.do_sync(false).await.unwrap();
    println!("{}", recipient.do_list_transactions().await.pretty(2));

    let connector = DarksideConnector(server_id.clone());
    let mut streamed_raw_txns = connector.get_incoming_transactions().await.unwrap();
    let raw_tx = streamed_raw_txns.message().await.unwrap().unwrap();
    // There should only be one transaction incoming
    assert!(streamed_raw_txns.message().await.unwrap().is_none());
    connector
        .stage_transactions_stream(vec![(raw_tx.data.clone(), 4)])
        .await
        .unwrap();
    connector.stage_blocks_create(4, 1, 0).await.unwrap();
    update_tree_states_for_transaction(&server_id, raw_tx.clone(), 4).await;
    connector.apply_staged(4).await.unwrap();
    sleep(std::time::Duration::from_secs(1)).await;

    recipient.do_sync(false).await.unwrap();
    //  light_client.do_sync(false).await.unwrap();
    println!(
        "Recipient pre-reorg: {}",
        recipient.do_list_transactions().await.pretty(2)
    );
    println!(
        "Recipient pre-reorg: {}",
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
    );
    println!(
        "Sender pre-reorg (unsynced): {}",
        serde_json::to_string_pretty(&light_client.do_balance().await).unwrap()
    );

    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();
    let connector = DarksideConnector(server_id.clone());
    connector.stage_blocks_create(4, 1, 0).await.unwrap();
    connector.apply_staged(4).await.unwrap();
    sleep(std::time::Duration::from_secs(1)).await;

    recipient.do_sync(false).await.unwrap();
    light_client.do_sync(false).await.unwrap();
    println!(
        "Recipient post-reorg: {}",
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
    );
    println!(
        "Sender post-reorg: {}",
        serde_json::to_string_pretty(&light_client.do_balance().await).unwrap()
    );
    println!(
        "Sender post-reorg: {}",
        light_client.do_list_transactions().await.pretty(2)
    );
    let loaded_client = light_client.new_client_from_save_buffer().await.unwrap();
    loaded_client.do_sync(false).await.unwrap();
    println!(
        "Sender post-load: {}",
        loaded_client.do_list_transactions().await.pretty(2)
    );
    assert_eq!(
        loaded_client.do_balance().await.orchard_balance,
        Some(100000000)
    );
}

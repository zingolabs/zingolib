tonic::include_proto!("cash.z.wallet.sdk.rpc");

use crate::{
    darkside::{constants::DARKSIDE_SEED, utils::DarksideHandler},
    utils::scenarios::setup::ClientManager,
};
use darkside_streamer_client::DarksideStreamerClient;
use json::JsonValue;
use tokio::time::sleep;

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
    ($($name:ident (&$self:ident, $($param:ident: $param_type:ty),*$(,)?) {$param_packing:expr}),*) => {$(
        #[allow(unused)]
        pub(crate) async fn $name(&$self, $($param: $param_type),*) -> ::std::result::Result<(), String> {
            let request = ::tonic::Request::new($param_packing);

            let mut client = $self.get_client().await.map_err(|e| format!("{e}"))?;

        let _response = client
            .$name(request)
            .await
            .map_err(|e| format!("{}", e))?
            .into_inner();

        Ok(())
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
        apply_staged(&self, height: i32) { DarksideHeight { height } },
        add_tree_state(&self, tree_state: TreeState) { tree_state },
        reset(
            &self,
            sapling_activation: i32,
            branch_id: String,
            chain_name: String,
        ) {
            DarksideMetaState {
                sapling_activation,
                branch_id,
                chain_name,
            }
        },
        stage_blocks(&self, url: String) { DarksideBlocksUrl { url } },
        stage_blocks_create(
            &self,
            height: i32,
            count: i32,
            nonce: i32
        ) {
            DarksideEmptyBlocks {
                height,
                count,
                nonce
            }
        },
        stage_blocks_stream(&self, blocks: Vec<String>) {
            ::futures_util::stream::iter(
                blocks.into_iter().map(|block| DarksideBlock { block })
            )
        }
    );
}

async fn prepare_darksidewalletd(uri: http::Uri) -> Result<(), String> {
    dbg!(&uri);
    let connector = DarksideConnector(uri);

    let mut client = connector.get_client().await.unwrap();
    // Setup prodedures.  Up to this point there's no communication between the client and the dswd
    client.clear_address_utxo(Empty {}).await.unwrap();

    // reset with parameters
    connector
        .reset(1, String::from("2bb40e60"), String::from("regtest"))
        .await
        .unwrap();

    connector
        .stage_blocks_stream(vec![String::from(
            crate::darkside::constants::GENESIS_BLOCK,
        )])
        .await?;

    connector.stage_blocks_create(2, 2, 0).await.unwrap();

    sleep(std::time::Duration::new(2, 0)).await;

    connector.apply_staged(3).await?;

    Ok(())
}

#[tokio::test]
async fn test_simple_sync() {
    let darkside_handler = DarksideHandler::new();

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone()).await.unwrap();

    let light_client = ClientManager::new(
        server_id,
        darkside_handler.darkside_dir.clone(),
        DARKSIDE_SEED,
    )
    .build_new_faucet(1, true)
    .await;

    let result = light_client.do_sync(true).await.unwrap();

    println!("{}", result);
    assert!(result.has_key("result"));
    let JsonValue::Object(res_value) = result
        else { panic!("Expected object, got {result:?}") };

    assert_eq!(res_value["result"], "success");
    assert_eq!(res_value["latest_block"], 3);
    assert_eq!(res_value["total_blocks_synced"], 3);
}

#[tokio::test]
async fn reorg_away_send() {
    let darkside_handler = DarksideHandler::new();

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone()).await.unwrap();

    let light_client = ClientManager::new(
        server_id,
        darkside_handler.darkside_dir.clone(),
        DARKSIDE_SEED,
    )
    .build_new_faucet(663150, true)
    .await;

    light_client.do_sync(true).await.unwrap();
    println!("{}", light_client.do_balance().await.pretty(4));
}

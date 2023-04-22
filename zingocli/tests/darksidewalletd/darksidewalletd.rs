tonic::include_proto!("cash.z.wallet.sdk.rpc");

use crate::utils::scenarios::setup::ClientManager;
use json::JsonValue;
use rand::Rng;
use tempdir::{self, TempDir};
use darkside_streamer_client::DarksideStreamerClient;
use tokio::time::sleep;

use std::sync::Arc;

use http_body::combinators::UnsyncBoxBody;
use hyper::{client::HttpConnector, Uri};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tonic::Request;
use tonic::Status;
use tower::{util::BoxCloneService, ServiceExt};

type UnderlyingService = BoxCloneService<
    http::Request<UnsyncBoxBody<prost::bytes::Bytes, Status>>,
    http::Response<hyper::Body>,
    hyper::Error,
>;

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
            if uri.scheme_str() == Some("https") {
                let mut roots = RootCertStore::empty();
                roots.add_server_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(
                    |anchor_ref| {
                        tokio_rustls::rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
                            anchor_ref.subject,
                            anchor_ref.spki,
                            anchor_ref.name_constraints,
                        )
                    },
                ));

                let tls = ClientConfig::builder()
                    .with_safe_defaults()
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                let connector = tower::ServiceBuilder::new()
                    .layer_fn(move |s| {
                        let tls = tls.clone();

                        hyper_rustls::HttpsConnectorBuilder::new()
                            .with_tls_config(tls)
                            .https_or_http()
                            .enable_http2()
                            .wrap_connector(s)
                    })
                    .service(http_connector);
                let client = Box::new(hyper::Client::builder().build(connector));
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
            } else {
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
    }

    pub(crate) async fn applyStaged(&self, height: i32) -> Result<(), String> {
        let height = DarksideHeight { height };
        let request = tonic::Request::new(height);

        let mut client = self.get_client().await.map_err(|e| format!("{}", e))?;

        let response = client
            .apply_staged(request)
            .await
            .map_err(|e| format!("{}", e))?
            .into_inner();

        Ok(())
    }

    pub(crate) async fn addTreeState(&self, treeState: TreeState) -> Result<(), String> {
        let request = tonic::Request::new(treeState);

        let mut client = self.get_client().await.map_err(|e| format!("{}", e))?;

        let response = client
            .add_tree_state(request)
            .await
            .map_err(|e| format!("{}", e))?
            .into_inner();

        Ok(())
    }

    pub(crate) async fn reset(&self, sapling_activation: i32, branch_id: String, chain_name: String) -> Result<(), String> {
        let mut reset_params = DarksideMetaState {
            sapling_activation,
            branch_id,
            chain_name
        };

        let mut request: Request<DarksideMetaState> = tonic::Request::new(reset_params);

        let mut client = self.get_client().await.map_err(|e| format!("{}", e))?;

        let mut response = client.reset(request)
            .await
            .map_err(|e| format!("{}", e))?
            .into_inner();

        Ok(())
    }

    pub(crate) async fn stageBlocks(&self, url: String) -> Result<(), String> {
        let url = DarksideBlocksUrl {
            url    
        };

        let request = tonic::Request::new(url);

        let mut client = self.get_client().await.map_err(|e| format!("{}", e))?;

        let response = client
            .stage_blocks(request)
            .await
            .map_err(|e| format!("{}", e))?
            .into_inner();

        Ok(())
    }


}

async fn prepare_darksidewalletd(uri: http::Uri) -> Result<(), String> {
    let connector = DarksideConnector(uri);

    // reset with parameters
    connector.reset(663_150, String::from("2bb40e60"), String::from("main")).await?;

    connector.stageBlocks(String::from("https://raw.githubusercontent.com/zcash-hackworks/darksidewalletd-test-data/master/tx-height-reorg/before-reorg.txt")).await?;
    
    sleep(std::time::Duration::new(2, 0)).await;
    
    let sapling_activation_tree = TreeState {
        network: String::from("main"),
        height: 663150,
        hash: String::from("0000000002fd3be4c24c437bd22620901617125ec2a3a6c902ec9a6c06f734fc"),
        time: 1576821833,
        sapling_tree: String::from("01ec6278a1bed9e1b080fd60ef50eb17411645e3746ff129283712bc4757ecc833001001b4e1d4a26ac4a2810b57a14f4ffb69395f55dde5674ecd2462af96f9126e054701a36afb68534f640938bdffd80dfcb3f4d5e232488abbf67d049b33a761e7ed6901a16e35205fb7fe626a9b13fc43e1d2b98a9c241f99f93d5e93a735454073025401f5b9bcbf3d0e3c83f95ee79299e8aeadf30af07717bda15ffb7a3d00243b58570001fa6d4c2390e205f81d86b85ace0b48f3ce0afb78eeef3e14c70bcfd7c5f0191c0000011bc9521263584de20822f9483e7edb5af54150c4823c775b2efc6a1eded9625501a6030f8d4b588681eddb66cad63f09c5c7519db49500fc56ebd481ce5e903c22000163f4eec5a2fe00a5f45e71e1542ff01e937d2210c99f03addcce5314a5278b2d0163ab01f46a3bb6ea46f5a19d5bdd59eb3f81e19cfa6d10ab0fd5566c7a16992601fa6980c053d84f809b6abcf35690f03a11f87b28e3240828e32e3f57af41e54e01319312241b0031e3a255b0d708750b4cb3f3fe79e3503fe488cc8db1dd00753801754bb593ea42d231a7ddf367640f09bbf59dc00f2c1d2003cc340e0c016b5b13"),
        orchard_tree: String::from(""),
    };

    connector.addTreeState(sapling_activation_tree).await?;
    
    let first_transaction_state = TreeState {
        network: String::from("main"),
        height: 663173,
        hash: String::from("0000000001b80f69c0b4a02228b59f2793ba9a4c3e8742fe56b9c9e331a6005d"),
        time: 1576823459,
        sapling_tree: String::from("018310f940d4b52399e656d8a090bd87f64a4abe80fffb2b4a882675def41f943c0114dd70a8a8a22869785b86b1dcad5e9508f419aad84ef8e83d50ec061117022310000199517be06af7c07c2d393c590c62add4dbcd9cc3402167521786f91a5d01d538000001989561014441f9f9043e11c01e220730df2219c090fa02f58d278fb7f447271601fa6d4c2390e205f81d86b85ace0b48f3ce0afb78eeef3e14c70bcfd7c5f0191c0000011bc9521263584de20822f9483e7edb5af54150c4823c775b2efc6a1eded9625501a6030f8d4b588681eddb66cad63f09c5c7519db49500fc56ebd481ce5e903c22000163f4eec5a2fe00a5f45e71e1542ff01e937d2210c99f03addcce5314a5278b2d0163ab01f46a3bb6ea46f5a19d5bdd59eb3f81e19cfa6d10ab0fd5566c7a16992601fa6980c053d84f809b6abcf35690f03a11f87b28e3240828e32e3f57af41e54e01319312241b0031e3a255b0d708750b4cb3f3fe79e3503fe488cc8db1dd00753801754bb593ea42d231a7ddf367640f09bbf59dc00f2c1d2003cc340e0c016b5b13"),
        orchard_tree: String::from(""),
    };

    connector.addTreeState(first_transaction_state).await?;

    let second_transaction_state = TreeState {
        network: String::from("main"),
        height: 663187,
        hash: String::from("00000000027fa4bfd6c012325a44eef7211a6162b5979507f07603333e9b3068"),
        time: 1576824317,
        sapling_tree: String::from("018d5b2b12a89cabbeb6c98bde09fbea143d49ea8e4dbf1d612e4906e73e1af96b001000018b01c7e7b2b183d022fc35e351e6423aee8885debc899036d0bc3b389c9f161501dba0595ce728b41452a9c595341074cf01e1152abe401db2b30a9ab007ad006e0001989561014441f9f9043e11c01e220730df2219c090fa02f58d278fb7f447271601fa6d4c2390e205f81d86b85ace0b48f3ce0afb78eeef3e14c70bcfd7c5f0191c0000011bc9521263584de20822f9483e7edb5af54150c4823c775b2efc6a1eded9625501a6030f8d4b588681eddb66cad63f09c5c7519db49500fc56ebd481ce5e903c22000163f4eec5a2fe00a5f45e71e1542ff01e937d2210c99f03addcce5314a5278b2d0163ab01f46a3bb6ea46f5a19d5bdd59eb3f81e19cfa6d10ab0fd5566c7a16992601fa6980c053d84f809b6abcf35690f03a11f87b28e3240828e32e3f57af41e54e01319312241b0031e3a255b0d708750b4cb3f3fe79e3503fe488cc8db1dd00753801754bb593ea42d231a7ddf367640f09bbf59dc00f2c1d2003cc340e0c016b5b13"),
        orchard_tree: String::from(""),
    };

    connector.addTreeState(second_transaction_state).await?;

    let third_transaction_state = TreeState {
        network: String::from("main"),
        height: 663189,
        hash: String::from("00000000025a03b6b11364b9c01a5fc9a927eca98ae899bb88f0ddf2de77fda3"),
        time: 1576824596,
        sapling_tree: String::from("01f1c5f08a96b8e0befe04ff061dd51ed96ffcc7fcd96d011dc06255692989731c001001ea6dbe4e95ec503600ee3260105ffcc2ceb63eb34ec323c794eebfc49b7beb2c018b01c7e7b2b183d022fc35e351e6423aee8885debc899036d0bc3b389c9f161501dba0595ce728b41452a9c595341074cf01e1152abe401db2b30a9ab007ad006e0001989561014441f9f9043e11c01e220730df2219c090fa02f58d278fb7f447271601fa6d4c2390e205f81d86b85ace0b48f3ce0afb78eeef3e14c70bcfd7c5f0191c0000011bc9521263584de20822f9483e7edb5af54150c4823c775b2efc6a1eded9625501a6030f8d4b588681eddb66cad63f09c5c7519db49500fc56ebd481ce5e903c22000163f4eec5a2fe00a5f45e71e1542ff01e937d2210c99f03addcce5314a5278b2d0163ab01f46a3bb6ea46f5a19d5bdd59eb3f81e19cfa6d10ab0fd5566c7a16992601fa6980c053d84f809b6abcf35690f03a11f87b28e3240828e32e3f57af41e54e01319312241b0031e3a255b0d708750b4cb3f3fe79e3503fe488cc8db1dd00753801754bb593ea42d231a7ddf367640f09bbf59dc00f2c1d2003cc340e0c016b5b13"),
        orchard_tree: String::from(""),
    };

    connector.addTreeState(third_transaction_state).await?;

    let fourth_transaction_state = TreeState {
        network: String::from("main"),
        height: 663194,
        hash: String::from("000000000096e4f59faae478c0ea5cb982c1cba223a92d15fbcf74795a0b4e5d"),
        time: 1576824759,
        sapling_tree: String::from("01f1c5f08a96b8e0befe04ff061dd51ed96ffcc7fcd96d011dc06255692989731c001001ea6dbe4e95ec503600ee3260105ffcc2ceb63eb34ec323c794eebfc49b7beb2c018b01c7e7b2b183d022fc35e351e6423aee8885debc899036d0bc3b389c9f161501dba0595ce728b41452a9c595341074cf01e1152abe401db2b30a9ab007ad006e0001989561014441f9f9043e11c01e220730df2219c090fa02f58d278fb7f447271601fa6d4c2390e205f81d86b85ace0b48f3ce0afb78eeef3e14c70bcfd7c5f0191c0000011bc9521263584de20822f9483e7edb5af54150c4823c775b2efc6a1eded9625501a6030f8d4b588681eddb66cad63f09c5c7519db49500fc56ebd481ce5e903c22000163f4eec5a2fe00a5f45e71e1542ff01e937d2210c99f03addcce5314a5278b2d0163ab01f46a3bb6ea46f5a19d5bdd59eb3f81e19cfa6d10ab0fd5566c7a16992601fa6980c053d84f809b6abcf35690f03a11f87b28e3240828e32e3f57af41e54e01319312241b0031e3a255b0d708750b4cb3f3fe79e3503fe488cc8db1dd00753801754bb593ea42d231a7ddf367640f09bbf59dc00f2c1d2003cc340e0c016b5b13"),
        orchard_tree: String::from(""),
    };

    connector.addTreeState(fourth_transaction_state).await?;
    
    sleep(std::time::Duration::new(2, 0)).await;
    
    connector.applyStaged(663_200).await?;
    
    Ok(())
}

#[tokio::test]
async fn test_simple_sync() {
    let mut rng = rand::thread_rng();

    let num: u32 = rng.gen_range(0..100000);
    let temp_dir = TempDir::new(&format!("dwld_test_{num}")).unwrap();
    let path = temp_dir.path().to_path_buf();

    let darkside_server_uri = zingoconfig::construct_lightwalletd_uri(Some(format!("http://127.0.0.1:9067")));

    prepare_darksidewalletd(darkside_server_uri).await;

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!("http://127.0.0.1:9067")));
    
    let light_client = ClientManager::new(
            server_id,
            path,
            "still champion voice habit trend flight survey between bitter process artefact blind carbon truly provide dizzy crush flush breeze blouse charge solid fish spread"
        )
            .build_new_faucet(663150, true).await;

    let result = light_client.do_sync(true).await.unwrap();

    println!("{}", result);
    assert!(result.has_key("result"));
    let res_value = match result {
        JsonValue::Object(res) => res,
        _ => panic!("Expected and object got something else"),
    };

    assert_eq!(res_value["result"], "success");
    assert_eq!(res_value["latest_block"], 663200);
    assert_eq!(res_value["total_blocks_synced"], 50);
}

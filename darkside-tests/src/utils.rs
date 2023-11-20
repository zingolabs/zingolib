use std::{
    path::PathBuf,
    process::{Child, Command},
    sync::Arc,
    time::Duration,
};

use http::Uri;
use http_body::combinators::UnsyncBoxBody;
use hyper::client::HttpConnector;
use orchard::{note_encryption::OrchardDomain, tree::MerkleHashOrchard};
use tempdir;
use tokio::time::sleep;
use tonic::Status;
use tower::{util::BoxCloneService, ServiceExt};
use zcash_primitives::merkle_tree::read_commitment_tree;
use zcash_primitives::sapling::{note_encryption::SaplingDomain, Node};
use zingo_testutils::{
    self,
    incrementalmerkletree::frontier::CommitmentTree,
    regtest::{get_cargo_manifest_dir, launch_lightwalletd},
};
use zingolib::wallet::traits::DomainWalletExt;

use crate::{
    constants::BRANCH_ID,
    darkside_types::{
        darkside_streamer_client::DarksideStreamerClient, DarksideBlock, DarksideBlocksUrl,
        DarksideEmptyBlocks, DarksideHeight, DarksideMetaState, Empty,
    },
};

use super::{
    constants,
    darkside_types::{RawTransaction, TreeState},
};

type UnderlyingService = BoxCloneService<
    http::Request<UnsyncBoxBody<prost::bytes::Bytes, Status>>,
    http::Response<hyper::Body>,
    hyper::Error,
>;

macro_rules! define_darkside_connector_methods(
    ($($name:ident (&$self:ident $(,$param:ident: $param_type:ty)*$(,)?) -> $return:ty {$param_packing:expr}),*) => {$(
        #[allow(unused)]
        pub async fn $name(&$self, $($param: $param_type),*) -> ::std::result::Result<$return, String> {
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
pub struct DarksideConnector(pub http::Uri);

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

pub async fn prepare_darksidewalletd(
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
        .stage_blocks_stream(vec![String::from(crate::constants::GENESIS_BLOCK)])
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
pub fn generate_darksidewalletd() -> (String, PathBuf) {
    let darkside_grpc_port = portpicker::pick_unused_port()
        .expect("Port unpickable!")
        .to_string();
    let darkside_dir = tempdir::TempDir::new("zingo_darkside_test")
        .unwrap()
        .into_path();
    (darkside_grpc_port, darkside_dir)
}

pub struct DarksideHandler {
    pub lightwalletd_handle: Child,
    pub grpc_port: String,
    pub darkside_dir: PathBuf,
}

impl Default for DarksideHandler {
    fn default() -> Self {
        Self::new()
    }
}
impl DarksideHandler {
    pub fn new() -> Self {
        let (grpc_port, darkside_dir) = generate_darksidewalletd();
        let grpc_bind_addr = Some(format!("127.0.0.1:{grpc_port}"));

        let check_interval = Duration::from_millis(300);
        let lightwalletd_handle = launch_lightwalletd(
            darkside_dir.join("logs"),
            darkside_dir.join("conf"),
            darkside_dir.join("data"),
            get_cargo_manifest_dir().join("lightwalletd_bin"),
            check_interval,
            grpc_bind_addr,
        );
        Self {
            lightwalletd_handle,
            grpc_port,
            darkside_dir,
        }
    }
}

impl Drop for DarksideHandler {
    fn drop(&mut self) {
        if Command::new("kill")
            .arg(self.lightwalletd_handle.id().to_string())
            .output()
            .is_err()
        {
            // if regular kill doesn't work, kill it harder
            let _ = self.lightwalletd_handle.kill();
        }
    }
}

pub async fn update_tree_states_for_transaction(
    server_id: &Uri,
    raw_tx: RawTransaction,
    height: u64,
) -> TreeState {
    let trees = zingolib::grpc_connector::GrpcConnector::get_trees(server_id.clone(), height - 1)
        .await
        .unwrap();
    let mut sapling_tree: zcash_primitives::sapling::CommitmentTree = read_commitment_tree(
        hex::decode(SaplingDomain::get_tree(&trees))
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    let mut orchard_tree: CommitmentTree<MerkleHashOrchard, 32> = read_commitment_tree(
        hex::decode(OrchardDomain::get_tree(&trees))
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    let transaction = zcash_primitives::transaction::Transaction::read(
        raw_tx.data.as_slice(),
        zcash_primitives::consensus::BranchId::Nu5,
    )
    .unwrap();
    for output in transaction
        .sapling_bundle()
        .iter()
        .flat_map(|bundle| bundle.shielded_outputs())
    {
        sapling_tree.append(Node::from_cmu(output.cmu())).unwrap()
    }
    for action in transaction
        .orchard_bundle()
        .iter()
        .flat_map(|bundle| bundle.actions())
    {
        orchard_tree
            .append(MerkleHashOrchard::from_cmx(action.cmx()))
            .unwrap()
    }
    let mut sapling_tree_bytes = vec![];
    zcash_primitives::merkle_tree::write_commitment_tree(&sapling_tree, &mut sapling_tree_bytes)
        .unwrap();
    let mut orchard_tree_bytes = vec![];
    zcash_primitives::merkle_tree::write_commitment_tree(&orchard_tree, &mut orchard_tree_bytes)
        .unwrap();
    let new_tree_state = TreeState {
        height,
        sapling_tree: hex::encode(sapling_tree_bytes),
        orchard_tree: hex::encode(orchard_tree_bytes),
        network: constants::first_tree_state().network,
        hash: "".to_string(),
        time: 0,
    };
    DarksideConnector::new(server_id.clone())
        .add_tree_state(new_tree_state.clone())
        .await
        .unwrap();
    new_tree_state
}

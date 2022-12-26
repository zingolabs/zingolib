use crate::apply_scenario;
use crate::blaze::block_witness_data::{
    update_trees_with_compact_transaction, CommitmentTreesForBlock,
};
use crate::blaze::test_utils::FakeCompactBlockList;
use crate::compact_formats::compact_tx_streamer_server::CompactTxStreamer;
use crate::compact_formats::compact_tx_streamer_server::CompactTxStreamerServer;
use crate::compact_formats::{
    Address, AddressList, Balance, BlockId, BlockRange, ChainSpec, CompactBlock, CompactTx,
    Duration, Empty, Exclude, GetAddressUtxosArg, GetAddressUtxosReply, GetAddressUtxosReplyList,
    LightdInfo, PingResponse, RawTransaction, SendResponse, TransparentAddressBlockFilter,
    TreeState, TxFilter,
};
use crate::wallet::data::TransactionMetadata;
use futures::{FutureExt, Stream};
use rand::rngs::OsRng;
use rand::Rng;
use std::cmp;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio_rustls::rustls::ServerConfig;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus::BranchId;
use zcash_primitives::merkle_tree::CommitmentTree;
use zcash_primitives::transaction::{Transaction, TxId};
use zingoconfig::{ChainType, ZingoConfig};

use super::LightClient;

pub(crate) const TEST_PEMFILE_PATH: &'static str = "test-data/localhost.pem";
static KEYGEN: std::sync::Once = std::sync::Once::new();

fn generate_tls_cert_and_key() -> (
    tokio_rustls::rustls::Certificate,
    tokio_rustls::rustls::PrivateKey,
) {
    use std::fs::File;
    use std::io::BufReader;
    KEYGEN.call_once(|| {
        std::process::Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-out",
                TEST_PEMFILE_PATH,
                "-keyout",
                TEST_PEMFILE_PATH,
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-sha256",
                "-subj",
                "/CN=localhost",
                "-extensions",
                "EXT",
                "-config",
                "test-data/openssl_cfg",
            ])
            .output()
            .unwrap();
        //tracing_subscriber::fmt::init();
    });
    (
        tokio_rustls::rustls::Certificate(
            rustls_pemfile::certs(&mut BufReader::new(File::open(TEST_PEMFILE_PATH).unwrap()))
                .unwrap()
                .pop()
                .unwrap(),
        ),
        tokio_rustls::rustls::PrivateKey(
            rustls_pemfile::pkcs8_private_keys(&mut BufReader::new(
                File::open(TEST_PEMFILE_PATH).unwrap(),
            ))
            .unwrap()
            .pop()
            .expect("empty vec of private keys??"),
        ),
    )
}

fn generate_tls_server_config() -> tokio_rustls::rustls::ServerConfig {
    let (cert, key) = generate_tls_cert_and_key();
    let mut tls_server_config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    tls_server_config.alpn_protocols = vec![b"h2".to_vec()];
    tls_server_config
}

pub async fn create_test_server() -> (
    Arc<RwLock<TestServerData>>,
    ZingoConfig,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    JoinHandle<()>,
) {
    let port = portpicker::pick_unused_port().unwrap();
    let server_port = format!("127.0.0.1:{}", port);
    let uri = format!("https://{}", server_port);
    let addr: std::net::SocketAddr = server_port.parse().unwrap();

    let mut config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, None);
    *config.server_uri.write().unwrap() = uri.replace("127.0.0.1", "localhost").parse().unwrap();

    let (service, data) = TestGRPCService::new(config.clone());

    let (data_dir_transmitter, data_dir_receiver) = oneshot::channel();

    // Communication channels for the server spawning thread
    let (ready_transmitter, ready_receiver) = oneshot::channel();
    let (stop_transmitter, stop_receiver) = oneshot::channel();
    let mut stop_fused = stop_receiver.fuse();
    let server_spawn_thread = tokio::spawn(async move {
        let svc = CompactTxStreamerServer::new(service);

        // We create the temp dir here, so that we can clean it up after the test runs
        let temp_dir = tempfile::Builder::new()
            .prefix(&format!("test{}", port).as_str())
            .tempdir()
            .unwrap();

        // Send the path name. Do into_path() to preserve the temp directory
        data_dir_transmitter
            .send(
                temp_dir
                    .into_path()
                    .canonicalize()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string(),
            )
            .unwrap();

        let svc = Server::builder().add_service(svc).into_service();

        let mut http = hyper::server::conn::Http::new();
        http.http2_only(true);

        let nameuri: std::string::String = uri
            .replace("https://", "")
            .replace("http://", "")
            .parse()
            .unwrap();
        let listener = tokio::net::TcpListener::bind(nameuri).await.unwrap();
        let tls_server_config = generate_tls_server_config();
        let tls_acceptor = { Some(tokio_rustls::TlsAcceptor::from(Arc::new(tls_server_config))) };

        ready_transmitter.send(()).unwrap();
        loop {
            let mut accepted = Box::pin(listener.accept().fuse());
            let conn_addr = futures::select_biased!(
                _ = (&mut stop_fused).fuse() => break,
                conn_addr = accepted => conn_addr,
            );
            let (conn, _addr) = match conn_addr {
                Ok(incoming) => incoming,
                Err(e) => {
                    eprintln!("Error accepting connection: {}", e);
                    continue;
                }
            };

            let http = http.clone();
            let tls_acceptor = tls_acceptor.clone();
            let svc = svc.clone();

            tokio::spawn(async move {
                let svc = tower::ServiceBuilder::new().service(svc);

                let mut certificates = Vec::new();
                let https_conn = tls_acceptor
                    .unwrap()
                    .accept_with(conn, |info| {
                        if let Some(certs) = info.peer_certificates() {
                            for cert in certs {
                                certificates.push(cert.clone());
                            }
                        }
                    })
                    .await
                    .unwrap();

                #[allow(unused_must_use)]
                {
                    http.serve_connection(https_conn, svc).await;
                };
            });
        }

        println!("Server stopped");
    });

    log::info!("creating data dir");
    let data_dir = data_dir_receiver.await.unwrap();
    println!(
        "GRPC Server listening on: {}. With datadir {}",
        addr, data_dir
    );
    config.data_dir = Some(data_dir);

    (
        data,
        config,
        ready_receiver,
        stop_transmitter,
        server_spawn_thread,
    )
}

pub struct NBlockFCBLScenario {
    pub data: Arc<RwLock<TestServerData>>,
    pub lightclient: LightClient,
    pub fake_compactblock_list: FakeCompactBlockList,
    pub config: ZingoConfig,
}

/// This scenario is used as the start state for 14 separate tests!
/// They are:
///
pub async fn setup_n_block_fcbl_scenario(
    initial_num_two_tx_blocks: u64,
) -> (NBlockFCBLScenario, oneshot::Sender<()>, JoinHandle<()>) {
    let (data, config, ready_receiver, stop_transmitter, test_server_handle) =
        create_test_server().await;
    ready_receiver.await.unwrap();

    let lightclient = LightClient::test_new(&config, None, 0).await.unwrap();

    let mut fake_compactblock_list = FakeCompactBlockList::new(0);

    mine_numblocks_each_with_two_sap_txs(
        &mut fake_compactblock_list,
        &data,
        &lightclient,
        initial_num_two_tx_blocks,
    )
    .await;
    (
        NBlockFCBLScenario {
            data,
            lightclient,
            fake_compactblock_list,
            config,
        },
        stop_transmitter,
        test_server_handle,
    )
}
///  This serves as a useful check on the correct behavior of our widely
//   used `setup_ten_block_fcbl_scenario`.
#[tokio::test]
async fn test_direct_grpc_and_lightclient_blockchain_height_agreement() {
    let expected_lightdinfo_before_blockmining = "LightdInfo ".to_string()
        + "{ version: \"Test GRPC Server\","
        + " vendor: \"\","
        + " taddr_support: true,"
        + " chain_name: \"fakemainnet\","
        + " sapling_activation_height: 1,"
        + " consensus_branch_id: \"\","
        + " block_height: 0,"
        + " git_commit: \"\","
        + " branch: \"\","
        + " build_date: \"\","
        + " build_user: \"\","
        + " estimated_height: 0,"
        + " zcashd_build: \"\","
        + " zcashd_subversion: \"\" }";
    let expected_lightdinfo_after_blockmining = "LightdInfo ".to_string()
        + "{ version: \"Test GRPC Server\","
        + " vendor: \"\","
        + " taddr_support: true,"
        + " chain_name: \"fakemainnet\","
        + " sapling_activation_height: 1,"
        + " consensus_branch_id: \"\","
        + " block_height: 10,"
        + " git_commit: \"\","
        + " branch: \"\","
        + " build_date: \"\","
        + " build_user: \"\","
        + " estimated_height: 0,"
        + " zcashd_build: \"\","
        + " zcashd_subversion: \"\" }";
    let (data, config, ready_receiver, stop_transmitter, test_server_handle) =
        create_test_server().await;

    let uri = config.server_uri.clone();
    let mut client = crate::grpc_connector::GrpcConnector::new(uri.read().unwrap().clone())
        .get_client()
        .await
        .unwrap();

    //let info_getter = &mut client.get_lightd_info(Request::new(Empty {}));
    ready_receiver.await.unwrap();
    let lightclient = LightClient::test_new(&config, None, 0).await.unwrap();
    let mut fake_compactblock_list = FakeCompactBlockList::new(0);

    let observed_pre_answer = format!(
        "{:?}",
        client
            .get_lightd_info(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner()
    );
    assert_eq!(observed_pre_answer, expected_lightdinfo_before_blockmining);
    assert_eq!(lightclient.wallet.last_synced_height().await, 0);
    // Change system under test state (generating random blocks)
    mine_numblocks_each_with_two_sap_txs(&mut fake_compactblock_list, &data, &lightclient, 10)
        .await;
    let observed_post_answer = format!(
        "{:?}",
        client
            .get_lightd_info(Request::new(Empty {}))
            .await
            .unwrap()
            .into_inner()
    );
    assert_eq!(observed_post_answer, expected_lightdinfo_after_blockmining);
    assert_eq!(lightclient.wallet.last_synced_height().await, 10);
    clean_shutdown(stop_transmitter, test_server_handle).await;
}
/// stop_transmitter: issues the shutdown to the server thread
/// server_thread_handle: this is Ready when the server thread exits
/// The first command initiates shutdown the second ensures that the
/// server thread has exited before the clean_shutdown returns
pub async fn clean_shutdown(
    stop_transmitter: oneshot::Sender<()>,
    server_thread_handle: JoinHandle<()>,
) {
    stop_transmitter.send(()).unwrap();
    server_thread_handle.await.unwrap();
}

pub async fn mine_numblocks_each_with_two_sap_txs(
    fcbl: &mut FakeCompactBlockList,
    testserver_state: &Arc<RwLock<TestServerData>>,
    lc: &LightClient,
    num: u64,
) {
    testserver_state.write().await.add_blocks(
        fcbl.create_and_append_randtx_blocks(num)
            .into_compact_blocks(),
    );
    lc.do_sync(true).await.unwrap();
}

pub async fn mine_pending_blocks(
    fcbl: &mut FakeCompactBlockList,
    testserver_state: &Arc<RwLock<TestServerData>>,
    lc: &LightClient,
) {
    testserver_state
        .write()
        .await
        .add_blocks(fcbl.into_compact_blocks());
    testserver_state
        .write()
        .await
        .add_transactions(fcbl.into_transactions());

    lc.do_sync(true).await.unwrap();
}

#[derive(Debug)]
pub struct TestServerData {
    pub blocks: Vec<CompactBlock>,
    pub transactions: HashMap<TxId, (Vec<String>, RawTransaction)>,
    pub sent_transactions: Vec<RawTransaction>,
    pub config: ZingoConfig,
    pub zec_price: f64,
    pub tree_states: Vec<CommitmentTreesForBlock>,
}

impl TestServerData {
    pub fn new(config: ZingoConfig) -> Self {
        let data = Self {
            blocks: vec![],
            transactions: HashMap::new(),
            sent_transactions: vec![],
            config,
            zec_price: 140.5,
            tree_states: vec![],
        };

        data
    }

    pub fn add_transactions(&mut self, transactions: Vec<(Transaction, u64, Vec<String>)>) {
        for (transaction, height, taddrs) in transactions {
            let mut raw_transaction = RawTransaction::default();
            let mut data = vec![];
            transaction.write(&mut data).unwrap();
            raw_transaction.data = data;
            raw_transaction.height = height;
            self.transactions
                .insert(transaction.txid(), (taddrs, raw_transaction));
        }
    }

    ///  This methods expects blocks in more recent (higher to lower) to
    ///  older order.   This is because client syncronization can know
    ///  that recent funds are unspent if it has checked all subsequent
    ///  notes.
    pub fn add_blocks(&mut self, cbs: Vec<CompactBlock>) {
        if cbs.is_empty() {
            panic!("No blocks");
        }

        if cbs.len() > 1 {
            if cbs.first().unwrap().height < cbs.last().unwrap().height {
                panic!(
                    "Blocks are in the wrong order. First={} Last={}",
                    cbs.first().unwrap().height,
                    cbs.last().unwrap().height
                );
            }
        }

        if !self.blocks.is_empty() {
            if self.blocks.first().unwrap().height + 1 != cbs.last().unwrap().height {
                panic!(
                    "New blocks are in wrong order. expecting={}, got={}",
                    self.blocks.first().unwrap().height + 1,
                    cbs.last().unwrap().height
                );
            }
        }

        for blk in cbs.into_iter().rev() {
            let last_tree_states = self.tree_states.last();
            let mut sapling_tree = last_tree_states
                .map(|trees| trees.sapling_tree.clone())
                .unwrap_or(CommitmentTree::empty());
            let mut orchard_tree = last_tree_states
                .map(|trees| trees.orchard_tree.clone())
                .unwrap_or(CommitmentTree::empty());
            for transaction in &blk.vtx {
                update_trees_with_compact_transaction(
                    &mut sapling_tree,
                    &mut orchard_tree,
                    transaction,
                )
            }
            let tree_states = CommitmentTreesForBlock {
                block_height: blk.height,
                block_hash: BlockHash::from_slice(&blk.hash).to_string(),
                sapling_tree,
                orchard_tree,
            };
            self.tree_states.push(tree_states);
            self.blocks.insert(0, blk);
        }
    }
}

#[derive(Debug)]
pub struct TestGRPCService {
    data: Arc<RwLock<TestServerData>>,
}

impl TestGRPCService {
    pub fn new(config: ZingoConfig) -> (Self, Arc<RwLock<TestServerData>>) {
        let data = Arc::new(RwLock::new(TestServerData::new(config)));
        let s = Self { data: data.clone() };

        (s, data)
    }

    async fn wait_random() {
        let msecs = OsRng.gen_range(0..100);
        sleep(std::time::Duration::from_millis(msecs)).await;
    }
}

#[tonic::async_trait]
impl CompactTxStreamer for TestGRPCService {
    async fn get_latest_block(
        &self,
        _request: Request<ChainSpec>,
    ) -> Result<Response<BlockId>, Status> {
        Self::wait_random().await;

        match self
            .data
            .read()
            .await
            .blocks
            .iter()
            .max_by_key(|b| b.height)
        {
            Some(latest_block) => Ok(Response::new(BlockId {
                height: latest_block.height,
                hash: latest_block.hash.clone(),
            })),
            None => Err(Status::unavailable("No blocks")),
        }
    }

    async fn get_block(&self, request: Request<BlockId>) -> Result<Response<CompactBlock>, Status> {
        Self::wait_random().await;

        let height = request.into_inner().height;

        match self
            .data
            .read()
            .await
            .blocks
            .iter()
            .find(|b| b.height == height)
        {
            Some(b) => Ok(Response::new(b.clone())),
            None => Err(Status::unavailable(format!("Block {} not found", height))),
        }
    }

    type GetBlockRangeStream =
        Pin<Box<dyn Stream<Item = Result<CompactBlock, Status>> + Send + Sync>>;
    async fn get_block_range(
        &self,
        request: Request<BlockRange>,
    ) -> Result<Response<Self::GetBlockRangeStream>, Status> {
        let request = request.into_inner();
        let start = request.start.unwrap().height;
        let end = request.end.unwrap().height;

        let rev = start < end;

        let (transmitter, receiver) = mpsc::channel(self.data.read().await.blocks.len());

        let blocks = self.data.read().await.blocks.clone();
        tokio::spawn(async move {
            let (iter, min, max) = if rev {
                (blocks.iter().rev().map(|b| b.clone()).collect(), start, end)
            } else {
                (blocks, end, start)
            };
            for b in iter {
                if b.height >= min && b.height <= max {
                    Self::wait_random().await;
                    transmitter.send(Ok(b)).await.unwrap();
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
    }

    async fn get_transaction(
        &self,
        request: Request<TxFilter>,
    ) -> Result<Response<RawTransaction>, Status> {
        Self::wait_random().await;

        let transaction_id = TransactionMetadata::new_txid(&request.into_inner().hash);
        match self.data.read().await.transactions.get(&transaction_id) {
            Some((_taddrs, transaction)) => Ok(Response::new(transaction.clone())),
            None => Err(Status::invalid_argument(format!(
                "Can't find txid {}",
                transaction_id
            ))),
        }
    }

    async fn send_transaction(
        &self,
        request: Request<RawTransaction>,
    ) -> Result<Response<SendResponse>, Status> {
        let raw_transaction = request.into_inner();
        let transaction_id = Transaction::read(&raw_transaction.data[..], BranchId::Sapling)
            .unwrap()
            .txid();

        self.data
            .write()
            .await
            .sent_transactions
            .push(raw_transaction);
        Ok(Response::new(SendResponse {
            error_message: transaction_id.to_string(),
            error_code: 0,
        }))
    }

    type GetTaddressTxidsStream =
        Pin<Box<dyn Stream<Item = Result<RawTransaction, Status>> + Send + Sync>>;

    async fn get_taddress_txids(
        &self,
        request: Request<TransparentAddressBlockFilter>,
    ) -> Result<Response<Self::GetTaddressTxidsStream>, Status> {
        let buf_size = cmp::max(self.data.read().await.transactions.len(), 1);
        let (transmitter, receiver) = mpsc::channel(buf_size);

        let request = request.into_inner();
        let taddr = request.address;
        let start_block = request
            .range
            .as_ref()
            .unwrap()
            .start
            .as_ref()
            .unwrap()
            .height;
        let end_block = request.range.as_ref().unwrap().end.as_ref().unwrap().height;

        let transactions = self.data.read().await.transactions.clone();
        tokio::spawn(async move {
            let mut transactions_to_send = transactions
                .values()
                .filter_map(|(taddrs, raw_transaction)| {
                    if taddrs.contains(&taddr)
                        && raw_transaction.height >= start_block
                        && raw_transaction.height <= end_block
                    {
                        Some(raw_transaction.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            transactions_to_send.sort_by_key(|raw_transaction| raw_transaction.height);

            for raw_transaction in transactions_to_send {
                Self::wait_random().await;

                transmitter.send(Ok(raw_transaction)).await.unwrap();
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
    }

    async fn get_taddress_balance(
        &self,
        _request: Request<AddressList>,
    ) -> Result<Response<Balance>, Status> {
        todo!()
    }

    async fn get_taddress_balance_stream(
        &self,
        _request: Request<tonic::Streaming<Address>>,
    ) -> Result<Response<Balance>, Status> {
        todo!()
    }

    type GetMempoolTxStream = Pin<Box<dyn Stream<Item = Result<CompactTx, Status>> + Send + Sync>>;

    async fn get_mempool_tx(
        &self,
        _request: Request<Exclude>,
    ) -> Result<Response<Self::GetMempoolTxStream>, Status> {
        todo!()
    }

    async fn get_tree_state(
        &self,
        request: Request<BlockId>,
    ) -> Result<Response<TreeState>, Status> {
        Self::wait_random().await;

        let block = request.into_inner();
        println!("Getting tree state at {}", block.height);

        // See if it is manually set.
        if let Some(tree_state) = self
            .data
            .read()
            .await
            .tree_states
            .iter()
            .find(|trees| trees.block_height == block.height)
            .map(CommitmentTreesForBlock::to_tree_state)
        {
            return Ok(Response::new(tree_state));
        }

        let start_block = self
            .data
            .read()
            .await
            .blocks
            .last()
            .map(|b| b.height - 1)
            .unwrap_or(0);

        let start_trees = self
            .data
            .read()
            .await
            .tree_states
            .iter()
            .find(|trees| trees.block_height == start_block)
            .map(Clone::clone)
            .unwrap_or(CommitmentTreesForBlock::empty());

        let mut trees = self
            .data
            .read()
            .await
            .blocks
            .iter()
            .rev()
            .take_while(|cb| cb.height <= block.height)
            .fold(start_trees, |mut trees, cb| {
                for transaction in &cb.vtx {
                    update_trees_with_compact_transaction(
                        &mut trees.sapling_tree,
                        &mut trees.orchard_tree,
                        transaction,
                    )
                }

                trees
            });

        trees.block_hash = BlockHash::from_slice(
            &self
                .data
                .read()
                .await
                .blocks
                .iter()
                .find(|cb| cb.height == block.height)
                .expect(format!("Couldn't find block {}", block.height).as_str())
                .hash[..],
        )
        .to_string();
        trees.block_height = block.height;

        Ok(Response::new(trees.to_tree_state()))
    }

    async fn get_address_utxos(
        &self,
        _request: Request<GetAddressUtxosArg>,
    ) -> Result<Response<GetAddressUtxosReplyList>, Status> {
        todo!()
    }

    type GetAddressUtxosStreamStream =
        Pin<Box<dyn Stream<Item = Result<GetAddressUtxosReply, Status>> + Send + Sync>>;

    async fn get_address_utxos_stream(
        &self,
        _request: Request<GetAddressUtxosArg>,
    ) -> Result<Response<Self::GetAddressUtxosStreamStream>, Status> {
        todo!()
    }

    async fn get_lightd_info(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<LightdInfo>, Status> {
        Self::wait_random().await;

        let mut ld = LightdInfo::default();
        ld.version = format!("Test GRPC Server");
        ld.block_height = self
            .data
            .read()
            .await
            .blocks
            .iter()
            .map(|b| b.height)
            .max()
            .unwrap_or(0);
        ld.taddr_support = true;
        ld.chain_name = self.data.read().await.config.chain.to_string();
        ld.sapling_activation_height = self.data.read().await.config.sapling_activation_height();

        Ok(Response::new(ld))
    }

    async fn ping(&self, _request: Request<Duration>) -> Result<Response<PingResponse>, Status> {
        todo!()
    }

    type GetMempoolStreamStream =
        Pin<Box<dyn Stream<Item = Result<RawTransaction, Status>> + Send + Sync>>;

    async fn get_mempool_stream(
        &self,
        _request: tonic::Request<crate::compact_formats::Empty>,
    ) -> Result<tonic::Response<Self::GetMempoolStreamStream>, tonic::Status> {
        todo!()
    }
}
apply_scenario! {check_reorg_buffer_offset 1}
async fn check_reorg_buffer_offset(scenario: NBlockFCBLScenario) {
    let NBlockFCBLScenario { lightclient, .. } = scenario;
    assert_eq!(
        lightclient.wallet.get_anchor_height().await,
        1 - zingoconfig::REORG_BUFFER_OFFSET
    );
}

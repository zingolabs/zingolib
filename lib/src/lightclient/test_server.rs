use crate::blaze::test_utils::{tree_to_string, FakeCompactBlockList};
use crate::compact_formats::compact_transaction_streamer_server::CompactTransactionStreamer;
use crate::compact_formats::compact_transaction_streamer_server::CompactTransactionStreamerServer;
use crate::compact_formats::{
    Address, AddressList, Balance, BlockId, BlockRange, ChainSpec, CompactBlock, CompactTransaction, Duration, Empty,
    Exclude, GetAddressUtxosArg, GetAddressUtxosReply, GetAddressUtxosReplyList, LightdInfo, PingResponse,
    PriceRequest, PriceResponse, RawTransaction, SendResponse, TransactionFilter, TransparentAddressBlockFilter,
    TreeState,
};
use crate::lightwallet::data::WalletTx;
use crate::lightwallet::now;
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
use zcash_primitives::merkle_tree::CommitmentTree;
use zcash_primitives::sapling::Node;
use zcash_primitives::transaction::{Transaction, TxId};

use super::lightclient_config::LightClientConfig;
use super::LightClient;

pub async fn create_test_server(
    https: bool,
) -> (
    Arc<RwLock<TestServerData>>,
    LightClientConfig,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    JoinHandle<()>,
) {
    let (ready_transmitter, ready_receiver) = oneshot::channel();
    let (stop_transmitter, stop_receiver) = oneshot::channel();
    let mut stop_fused = stop_receiver.fuse();

    let port = portpicker::pick_unused_port().unwrap();
    let server_port = format!("127.0.0.1:{}", port);
    let uri = if https {
        format!("https://{}", server_port)
    } else {
        format!("http://{}", server_port)
    };
    let addr: std::net::SocketAddr = server_port.parse().unwrap();

    let mut config = LightClientConfig::create_unconnected("main".to_string(), None);
    config.server = uri.replace("127.0.0.1", "localhost").parse().unwrap();

    let (service, data) = TestGRPCService::new(config.clone());

    let (data_dir_transmitter, data_dir_receiver) = oneshot::channel();

    let h1 = tokio::spawn(async move {
        let svc = CompactTransactionStreamerServer::new(service);

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

        let mut tls;
        let svc = Server::builder().add_service(svc).into_service();

        let mut http = hyper::server::conn::Http::new();
        http.http2_only(true);

        let nameuri: std::string::String = uri.replace("https://", "").replace("http://", "").parse().unwrap();
        let listener = tokio::net::TcpListener::bind(nameuri).await.unwrap();
        let tls_acceptor = if https {
            let file = "localhost.pem";
            use std::fs::File;
            use std::io::BufReader;
            let (cert, key) = (
                tokio_rustls::rustls::Certificate(
                    rustls_pemfile::certs(&mut BufReader::new(File::open(file).unwrap()))
                        .unwrap()
                        .pop()
                        .unwrap(),
                ),
                tokio_rustls::rustls::PrivateKey(
                    rustls_pemfile::pkcs8_private_keys(&mut BufReader::new(File::open(file).unwrap()))
                        .unwrap()
                        .pop()
                        .expect("empty vec of private keys??"),
                ),
            );
            tls = ServerConfig::builder()
                .with_safe_defaults()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .unwrap();
            tls.alpn_protocols = vec![b"h2".to_vec()];
            Some(tokio_rustls::TlsAcceptor::from(Arc::new(tls)))
        } else {
            None
        };

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
                if https {
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
                } else {
                    #[allow(unused_must_use)]
                    {
                        http.serve_connection(conn, svc).await;
                    };
                }
            });
        }

        println!("Server stopped");
    });

    log::info!("creating data dir");
    let data_dir = data_dir_receiver.await.unwrap();
    println!("GRPC Server listening on: {}. With datadir {}", addr, data_dir);
    config.data_dir = Some(data_dir);

    (data, config, ready_receiver, stop_transmitter, h1)
}

pub async fn mine_random_blocks(
    fcbl: &mut FakeCompactBlockList,
    data: &Arc<RwLock<TestServerData>>,
    lc: &LightClient,
    num: u64,
) {
    let cbs = fcbl.add_blocks(num).into_compact_blocks();

    data.write().await.add_blocks(cbs.clone());
    lc.do_sync(true).await.unwrap();
}

pub async fn mine_pending_blocks(
    fcbl: &mut FakeCompactBlockList,
    data: &Arc<RwLock<TestServerData>>,
    lc: &LightClient,
) {
    let cbs = fcbl.into_compact_blocks();

    data.write().await.add_blocks(cbs.clone());
    let mut v = fcbl.into_transactions();

    // Add all the t-addr spend's t-addresses into the maps, so the test grpc server
    // knows to serve this transaction when the transactions for this particular taddr are requested.
    for (t, _h, taddrs) in v.iter_mut() {
        for vin in &t.vin {
            let prev_transaction_id = WalletTx::new_txid(&vin.prevout.hash().to_vec());
            if let Some(wtx) = lc.wallet.transactions.read().await.current.get(&prev_transaction_id) {
                if let Some(utxo) = wtx.utxos.iter().find(|u| u.output_index as u32 == vin.prevout.n()) {
                    if !taddrs.contains(&utxo.address) {
                        taddrs.push(utxo.address.clone());
                    }
                }
            }
        }
    }

    data.write().await.add_transactions(v);

    lc.do_sync(true).await.unwrap();
}

#[derive(Debug)]
pub struct TestServerData {
    pub blocks: Vec<CompactBlock>,
    pub transactions: HashMap<TxId, (Vec<String>, RawTransaction)>,
    pub sent_transactions: Vec<RawTransaction>,
    pub config: LightClientConfig,
    pub zec_price: f64,
    pub tree_states: Vec<(u64, String, String)>,
}

impl TestServerData {
    pub fn new(config: LightClientConfig) -> Self {
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
            self.transactions.insert(transaction.txid(), (taddrs, raw_transaction));
        }
    }

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
            self.blocks.insert(0, blk);
        }
    }
}

#[derive(Debug)]
pub struct TestGRPCService {
    data: Arc<RwLock<TestServerData>>,
}

impl TestGRPCService {
    pub fn new(config: LightClientConfig) -> (Self, Arc<RwLock<TestServerData>>) {
        let data = Arc::new(RwLock::new(TestServerData::new(config)));
        let s = Self { data: data.clone() };

        (s, data)
    }

    async fn wait_random() {
        let msecs = OsRng.gen_range(0, 100);
        sleep(std::time::Duration::from_millis(msecs)).await;
    }
}

#[tonic::async_trait]
impl CompactTransactionStreamer for TestGRPCService {
    async fn get_latest_block(&self, _request: Request<ChainSpec>) -> Result<Response<BlockId>, Status> {
        Self::wait_random().await;

        match self.data.read().await.blocks.iter().max_by_key(|b| b.height) {
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

        match self.data.read().await.blocks.iter().find(|b| b.height == height) {
            Some(b) => Ok(Response::new(b.clone())),
            None => Err(Status::unavailable(format!("Block {} not found", height))),
        }
    }

    type GetBlockRangeStream = Pin<Box<dyn Stream<Item = Result<CompactBlock, Status>> + Send + Sync>>;
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

    async fn get_zec_price(&self, _request: Request<PriceRequest>) -> Result<Response<PriceResponse>, Status> {
        self.get_current_zec_price(Request::new(Empty {})).await
    }

    async fn get_current_zec_price(&self, _request: Request<Empty>) -> Result<Response<PriceResponse>, Status> {
        Self::wait_random().await;

        let mut res = PriceResponse::default();
        res.currency = "USD".to_string();
        res.timestamp = now() as i64;
        res.price = self.data.read().await.zec_price;

        Ok(Response::new(res))
    }

    async fn get_transaction(&self, request: Request<TransactionFilter>) -> Result<Response<RawTransaction>, Status> {
        Self::wait_random().await;

        let transaction_id = WalletTx::new_txid(&request.into_inner().hash);
        match self.data.read().await.transactions.get(&transaction_id) {
            Some((_taddrs, transaction)) => Ok(Response::new(transaction.clone())),
            None => Err(Status::invalid_argument(format!("Can't find txid {}", transaction_id))),
        }
    }

    async fn send_transaction(&self, request: Request<RawTransaction>) -> Result<Response<SendResponse>, Status> {
        let raw_transaction = request.into_inner();
        let transaction_id = Transaction::read(&raw_transaction.data[..]).unwrap().txid();

        self.data.write().await.sent_transactions.push(raw_transaction);
        Ok(Response::new(SendResponse {
            error_message: transaction_id.to_string(),
            error_code: 0,
        }))
    }

    type GetTaddressTransactionIdsStream = Pin<Box<dyn Stream<Item = Result<RawTransaction, Status>> + Send + Sync>>;

    async fn get_taddress_transaction_ids(
        &self,
        request: Request<TransparentAddressBlockFilter>,
    ) -> Result<Response<Self::GetTaddressTransactionIdsStream>, Status> {
        let buf_size = cmp::max(self.data.read().await.transactions.len(), 1);
        let (transmitter, receiver) = mpsc::channel(buf_size);

        let request = request.into_inner();
        let taddr = request.address;
        let start_block = request.range.as_ref().unwrap().start.as_ref().unwrap().height;
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

    type GetAddressTransactionIdsStream = Pin<Box<dyn Stream<Item = Result<RawTransaction, Status>> + Send + Sync>>;

    async fn get_address_transaction_ids(
        &self,
        request: Request<TransparentAddressBlockFilter>,
    ) -> Result<Response<Self::GetAddressTransactionIdsStream>, Status> {
        self.get_taddress_transaction_ids(request).await
    }

    async fn get_taddress_balance(&self, _request: Request<AddressList>) -> Result<Response<Balance>, Status> {
        todo!()
    }

    async fn get_taddress_balance_stream(
        &self,
        _request: Request<tonic::Streaming<Address>>,
    ) -> Result<Response<Balance>, Status> {
        todo!()
    }

    type GetMempoolTransactionStream = Pin<Box<dyn Stream<Item = Result<CompactTransaction, Status>> + Send + Sync>>;

    async fn get_mempool_transaction(
        &self,
        _request: Request<Exclude>,
    ) -> Result<Response<Self::GetMempoolTransactionStream>, Status> {
        todo!()
    }

    async fn get_tree_state(&self, request: Request<BlockId>) -> Result<Response<TreeState>, Status> {
        Self::wait_random().await;

        let block = request.into_inner();
        println!("Getting tree state at {}", block.height);

        // See if it is manually set.
        if let Some((height, hash, tree)) = self
            .data
            .read()
            .await
            .tree_states
            .iter()
            .find(|(h, _, _)| *h == block.height)
        {
            let mut ts = TreeState::default();
            ts.height = *height;
            ts.hash = hash.clone();
            ts.tree = tree.clone();

            return Ok(Response::new(ts));
        }

        let start_block = self.data.read().await.blocks.last().map(|b| b.height - 1).unwrap_or(0);

        let start_tree = self
            .data
            .read()
            .await
            .tree_states
            .iter()
            .find(|(h, _, _)| *h == start_block)
            .map(|(_, _, t)| CommitmentTree::<Node>::read(&hex::decode(t).unwrap()[..]).unwrap())
            .unwrap_or(CommitmentTree::<Node>::empty());

        let tree = self
            .data
            .read()
            .await
            .blocks
            .iter()
            .rev()
            .take_while(|cb| cb.height <= block.height)
            .fold(start_tree, |mut tree, cb| {
                for transaction in &cb.v_transaction {
                    for co in &transaction.outputs {
                        tree.append(Node::new(co.cmu().unwrap().into())).unwrap();
                    }
                }

                tree
            });

        let mut ts = TreeState::default();
        ts.hash = BlockHash::from_slice(
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
        ts.height = block.height;
        ts.tree = tree_to_string(&tree);

        Ok(Response::new(ts))
    }

    async fn get_address_utxos(
        &self,
        _request: Request<GetAddressUtxosArg>,
    ) -> Result<Response<GetAddressUtxosReplyList>, Status> {
        todo!()
    }

    type GetAddressUtxosStreamStream = Pin<Box<dyn Stream<Item = Result<GetAddressUtxosReply, Status>> + Send + Sync>>;

    async fn get_address_utxos_stream(
        &self,
        _request: Request<GetAddressUtxosArg>,
    ) -> Result<Response<Self::GetAddressUtxosStreamStream>, Status> {
        todo!()
    }

    async fn get_lightd_info(&self, _request: Request<Empty>) -> Result<Response<LightdInfo>, Status> {
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
        ld.chain_name = self.data.read().await.config.chain_name.clone();
        ld.sapling_activation_height = self.data.read().await.config.sapling_activation_height;

        Ok(Response::new(ld))
    }

    async fn ping(&self, _request: Request<Duration>) -> Result<Response<PingResponse>, Status> {
        todo!()
    }

    type GetMempoolStreamStream = Pin<Box<dyn Stream<Item = Result<RawTransaction, Status>> + Send + Sync>>;

    async fn get_mempool_stream(
        &self,
        _request: tonic::Request<crate::compact_formats::Empty>,
    ) -> Result<tonic::Response<Self::GetMempoolStreamStream>, tonic::Status> {
        todo!()
    }
}

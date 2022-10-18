use std::sync::Arc;

use crate::compact_formats::compact_tx_streamer_client::CompactTxStreamerClient;
use crate::compact_formats::{
    BlockId, BlockRange, ChainSpec, CompactBlock, Empty, LightdInfo, RawTransaction,
    TransparentAddressBlockFilter, TreeState, TxFilter,
};
use futures::future::join_all;
use futures::stream::FuturesUnordered;
use futures::StreamExt;

use http_body::combinators::UnsyncBoxBody;
use hyper::{client::HttpConnector, Uri};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tonic::Request;
use tonic::Status;
use tower::{util::BoxCloneService, ServiceExt};
use zcash_primitives::consensus::{BlockHeight, BranchId, Parameters};
use zcash_primitives::transaction::{Transaction, TxId};

type UnderlyingService = BoxCloneService<
    http::Request<UnsyncBoxBody<prost::bytes::Bytes, Status>>,
    http::Response<hyper::Body>,
    hyper::Error,
>;

#[derive(Clone)]
pub struct GrpcConnector {
    uri: http::Uri,
}

impl GrpcConnector {
    pub fn new(uri: http::Uri) -> Self {
        Self { uri }
    }

    pub(crate) fn get_client(
        &self,
    ) -> impl std::future::Future<
        Output = Result<CompactTxStreamerClient<UnderlyingService>, Box<dyn std::error::Error>>,
    > {
        let uri = Arc::new(self.uri.clone());
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

                #[cfg(test)]
                add_test_cert_to_roots(&mut roots);

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

                Ok(CompactTxStreamerClient::new(svc.boxed_clone()))
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

                Ok(CompactTxStreamerClient::new(svc.boxed_clone()))
            }
        }
    }

    pub async fn start_saplingtree_fetcher(
        &self,
    ) -> (
        JoinHandle<()>,
        UnboundedSender<(u64, oneshot::Sender<Result<TreeState, String>>)>,
    ) {
        let (transmitter, mut receiver) =
            unbounded_channel::<(u64, oneshot::Sender<Result<TreeState, String>>)>();
        let uri = self.uri.clone();

        let h = tokio::spawn(async move {
            let uri = uri.clone();
            while let Some((height, result_transmitter)) = receiver.recv().await {
                result_transmitter
                    .send(Self::get_trees(uri.clone(), height).await)
                    .unwrap()
            }
        });

        (h, transmitter)
    }

    pub async fn start_taddr_transaction_fetcher(
        &self,
    ) -> (
        JoinHandle<()>,
        oneshot::Sender<(
            (Vec<String>, u64, u64),
            oneshot::Sender<Vec<UnboundedReceiver<Result<RawTransaction, String>>>>,
        )>,
    ) {
        let (transmitter, receiver) = oneshot::channel::<(
            (Vec<String>, u64, u64),
            oneshot::Sender<Vec<UnboundedReceiver<Result<RawTransaction, String>>>>,
        )>();
        let uri = self.uri.clone();

        let h = tokio::spawn(async move {
            let uri = uri.clone();
            if let Ok(((taddrs, start_height, end_height), result_transmitter)) = receiver.await {
                let mut transaction_receivers = vec![];
                let mut transaction_receivers_workers = vec![];

                // Create a stream for every t-addr
                for taddr in taddrs {
                    let (transaction_s, transaction_receiver) = unbounded_channel();
                    transaction_receivers.push(transaction_receiver);
                    transaction_receivers_workers.push(tokio::spawn(Self::get_taddr_transactions(
                        uri.clone(),
                        taddr,
                        start_height,
                        end_height,
                        transaction_s,
                    )));
                }

                // Dispatch a set of receivers
                result_transmitter.send(transaction_receivers).unwrap();

                // // Wait for all the t-addr transactions to be fetched from LightwalletD and sent to the h1 handle.
                join_all(transaction_receivers_workers).await;
            }
        });

        (h, transmitter)
    }

    pub async fn start_full_transaction_fetcher(
        &self,
        network: impl Parameters + Send + Copy + 'static,
    ) -> (
        JoinHandle<()>,
        UnboundedSender<(TxId, oneshot::Sender<Result<Transaction, String>>)>,
    ) {
        let (transmitter, mut receiver) =
            unbounded_channel::<(TxId, oneshot::Sender<Result<Transaction, String>>)>();
        let uri = self.uri.clone();

        let h = tokio::spawn(async move {
            let mut workers = FuturesUnordered::new();
            while let Some((transaction_id, result_transmitter)) = receiver.recv().await {
                let uri = uri.clone();
                workers.push(tokio::spawn(async move {
                    result_transmitter
                        .send(
                            Self::get_full_transaction(uri.clone(), &transaction_id, network).await,
                        )
                        .unwrap()
                }));

                // Do only 16 API calls in parallel, otherwise it might overflow OS's limit of
                // number of simultaneous connections
                if workers.len() > 16 {
                    while let Some(_r) = workers.next().await {
                        // Do nothing
                    }
                }
            }
        });

        (h, transmitter)
    }

    pub async fn get_block_range(
        &self,
        start_height: u64,
        end_height: u64,
        senders: &[UnboundedSender<CompactBlock>; 2],
    ) -> Result<(), String> {
        let mut client = self.get_client().await.map_err(|e| format!("{}", e))?;

        let bs = BlockId {
            height: start_height,
            hash: vec![],
        };
        let be = BlockId {
            height: end_height,
            hash: vec![],
        };

        let request = Request::new(BlockRange {
            start: Some(bs),
            end: Some(be),
        });

        let mut response = client
            .get_block_range(request)
            .await
            .map_err(|e| format!("{}", e))?
            .into_inner();

        while let Some(block) = response.message().await.map_err(|e| format!("{}", e))? {
            senders[0]
                .send(block.clone())
                .map_err(|e| format!("{}", e))?;
            senders[1].send(block).map_err(|e| format!("{}", e))?;
        }

        Ok(())
    }

    async fn get_full_transaction(
        uri: http::Uri,
        transaction_id: &TxId,
        network: impl Parameters,
    ) -> Result<Transaction, String> {
        let client = Arc::new(GrpcConnector::new(uri));
        let request = Request::new(TxFilter {
            block: None,
            index: 0,
            hash: transaction_id.as_ref().to_vec(),
        });

        // log::info!("Full fetching {}", transaction_id);

        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let response = client
            .get_transaction(request)
            .await
            .map_err(|e| format!("{}", e))?
            .into_inner();

        Transaction::read(
            &response.data[..],
            BranchId::for_height(&network, BlockHeight::from_u32(response.height as u32)),
        )
        .map_err(|e| format!("Error parsing Transaction: {}", e))
    }

    async fn get_taddr_transactions(
        uri: http::Uri,
        taddr: String,
        start_height: u64,
        end_height: u64,
        transactions_sender: UnboundedSender<Result<RawTransaction, String>>,
    ) -> Result<(), String> {
        let client = Arc::new(GrpcConnector::new(uri));

        // Make sure start_height is smaller than end_height, because the API expects it like that
        let (start_height, end_height) = if start_height < end_height {
            (start_height, end_height)
        } else {
            (end_height, start_height)
        };

        let start = Some(BlockId {
            height: start_height,
            hash: vec![],
        });
        let end = Some(BlockId {
            height: end_height,
            hash: vec![],
        });

        let args = TransparentAddressBlockFilter {
            address: taddr,
            range: Some(BlockRange { start, end }),
        };
        let request = Request::new(args.clone());

        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let maybe_response = match client.get_taddress_txids(request).await {
            Ok(r) => r,
            Err(e) => {
                if e.code() == tonic::Code::Unimplemented {
                    // Try the old, legacy API
                    let request = Request::new(args);
                    client
                        .get_taddress_txids(request)
                        .await
                        .map_err(|e| format!("{}", e))?
                } else {
                    return Err(format!("{}", e));
                }
            }
        };

        let mut response = maybe_response.into_inner();

        while let Some(transaction) = response.message().await.map_err(|e| format!("{}", e))? {
            transactions_sender.send(Ok(transaction)).unwrap();
        }

        Ok(())
    }

    pub async fn get_info(uri: http::Uri) -> Result<LightdInfo, String> {
        let client = Arc::new(GrpcConnector::new(uri.clone()));

        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let request = Request::new(Empty {});

        let response = client
            .get_lightd_info(request)
            .await
            .map_err(|e| format!("Error with get_lightd_info response at {uri}: {e:?}"))?;
        Ok(response.into_inner())
    }

    pub async fn monitor_mempool(
        uri: http::Uri,
        mempool_transmitter: UnboundedSender<RawTransaction>,
    ) -> Result<(), String> {
        let client = Arc::new(GrpcConnector::new(uri));

        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let request = Request::new(Empty {});

        let mut response = client
            .get_mempool_stream(request)
            .await
            .map_err(|e| format!("{}", e))?
            .into_inner();
        while let Some(r_transmitter) = response.message().await.map_err(|e| format!("{}", e))? {
            mempool_transmitter
                .send(r_transmitter)
                .map_err(|e| format!("{}", e))?;
        }

        Ok(())
    }

    pub async fn get_trees(uri: http::Uri, height: u64) -> Result<TreeState, String> {
        let client = Arc::new(GrpcConnector::new(uri.clone()));
        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let b = BlockId {
            height: height as u64,
            hash: vec![],
        };
        let response = client
            .get_tree_state(Request::new(b))
            .await
            .map_err(|e| format!("Error with get_tree_state response at {uri}: {:?}", e))?;

        Ok(response.into_inner())
    }

    // get_latest_block GRPC call
    pub async fn get_latest_block(uri: http::Uri) -> Result<BlockId, String> {
        let client = Arc::new(GrpcConnector::new(uri.clone()));
        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let request = Request::new(ChainSpec {});

        let response = client
            .get_latest_block(request)
            .await
            .map_err(|e| format!("Error with get_latest_block response at {uri}: {:?}", e))?;

        Ok(response.into_inner())
    }

    pub async fn send_transaction(
        uri: http::Uri,
        transaction_bytes: Box<[u8]>,
    ) -> Result<String, String> {
        let client = Arc::new(GrpcConnector::new(uri));
        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let request = Request::new(RawTransaction {
            data: transaction_bytes.to_vec(),
            height: 0,
        });

        let response = client
            .send_transaction(request)
            .await
            .map_err(|e| format!("Send Error: {}", e))?;

        let sendresponse = response.into_inner();
        if sendresponse.error_code == 0 {
            let mut transaction_id = sendresponse.error_message;
            if transaction_id.starts_with("\"") && transaction_id.ends_with("\"") {
                transaction_id = transaction_id[1..transaction_id.len() - 1].to_string();
            }

            Ok(transaction_id)
        } else {
            Err(format!("Error: {:?}", sendresponse))
        }
    }
}

#[cfg(test)]
fn add_test_cert_to_roots(roots: &mut RootCertStore) {
    let fd = std::fs::File::open(crate::lightclient::test_server::TEST_PEMFILE_PATH).unwrap();
    let mut buf = std::io::BufReader::new(&fd);
    let certs = rustls_pemfile::certs(&mut buf).unwrap();
    roots.add_parsable_certificates(&certs);
}

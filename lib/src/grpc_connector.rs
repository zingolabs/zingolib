use std::collections::HashMap;
use std::sync::Arc;

use crate::compact_formats::compact_transaction_streamer_client::CompactTransactionStreamerClient;
use crate::compact_formats::{
    BlockId, BlockRange, ChainSpec, CompactBlock, Empty, LightdInfo, PriceRequest, PriceResponse, RawTransaction,
    TransactionFilter, TransparentAddressBlockFilter, TreeState,
};
use futures::future::join_all;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use log::warn;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::ClientConfig;
use tonic::transport::ClientTlsConfig;
use tonic::{
    transport::{Channel, Error},
    Request,
};
use zcash_primitives::transaction::{Transaction, TxId};

#[derive(Clone)]
pub struct GrpcConnector {
    uri: http::Uri,
}

impl GrpcConnector {
    pub fn new(uri: http::Uri) -> Self {
        Self { uri }
    }

    pub(crate) async fn get_client(&self) -> Result<CompactTransactionStreamerClient<Channel>, Error> {
        let channel = if self.uri.scheme_str() == Some("http") {
            //println!("http");
            Channel::builder(self.uri.clone()).connect().await?
        } else {
            //println!("https");

            let tls = ClientTlsConfig::new().domain_name(self.uri.host().unwrap());

            #[cfg(test)]
            let tls = add_tls_test_config(tls).await;

            dbg!(&self.uri);
            println!("{:?}", tls);
            Channel::builder(self.uri.clone())
                .tls_config(tls)
                .unwrap()
                .connect()
                .await?
        };

        Ok(CompactTransactionStreamerClient::new(channel))
    }

    pub async fn start_saplingtree_fetcher(
        &self,
    ) -> (
        JoinHandle<()>,
        UnboundedSender<(u64, oneshot::Sender<Result<TreeState, String>>)>,
    ) {
        let (transmitter, mut receiver) = unbounded_channel::<(u64, oneshot::Sender<Result<TreeState, String>>)>();
        let uri = self.uri.clone();

        let h = tokio::spawn(async move {
            let uri = uri.clone();
            while let Some((height, result_transmitter)) = receiver.recv().await {
                result_transmitter
                    .send(Self::get_sapling_tree(uri.clone(), height).await)
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

                // Dispatch a set of recievers
                result_transmitter.send(transaction_receivers).unwrap();

                // // Wait for all the t-addr transactions to be fetched from LightwalletD and sent to the h1 handle.
                join_all(transaction_receivers_workers).await;
            }
        });

        (h, transmitter)
    }

    pub async fn start_full_transaction_fetcher(
        &self,
    ) -> (
        JoinHandle<()>,
        UnboundedSender<(TxId, oneshot::Sender<Result<Transaction, String>>)>,
    ) {
        let (transmitter, mut receiver) = unbounded_channel::<(TxId, oneshot::Sender<Result<Transaction, String>>)>();
        let uri = self.uri.clone();

        let h = tokio::spawn(async move {
            let mut workers = FuturesUnordered::new();
            while let Some((transaction_id, result_transmitter)) = receiver.recv().await {
                let uri = uri.clone();
                workers.push(tokio::spawn(async move {
                    result_transmitter
                        .send(Self::get_full_transaction(uri.clone(), &transaction_id).await)
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
            senders[0].send(block.clone()).map_err(|e| format!("{}", e))?;
            senders[1].send(block).map_err(|e| format!("{}", e))?;
        }

        Ok(())
    }

    async fn get_full_transaction(uri: http::Uri, transaction_id: &TxId) -> Result<Transaction, String> {
        let client = Arc::new(GrpcConnector::new(uri));
        let request = Request::new(TransactionFilter {
            block: None,
            index: 0,
            hash: transaction_id.0.to_vec(),
        });

        // log::info!("Full fetching {}", transaction_id);

        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let response = client.get_transaction(request).await.map_err(|e| format!("{}", e))?;

        Transaction::read(&response.into_inner().data[..]).map_err(|e| format!("Error parsing Transaction: {}", e))
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

        let maybe_response = match client.get_taddress_transaction_ids(request).await {
            Ok(r) => r,
            Err(e) => {
                if e.code() == tonic::Code::Unimplemented {
                    // Try the old, legacy API
                    let request = Request::new(args);
                    client
                        .get_address_transaction_ids(request)
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
        let client = Arc::new(GrpcConnector::new(uri));

        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let request = Request::new(Empty {});

        let response = client
            .get_lightd_info(request)
            .await
            .map_err(|e| format!("Error with response: {:?}", e))?;
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
            mempool_transmitter.send(r_transmitter).map_err(|e| format!("{}", e))?;
        }

        Ok(())
    }

    pub async fn get_sapling_tree(uri: http::Uri, height: u64) -> Result<TreeState, String> {
        let client = Arc::new(GrpcConnector::new(uri));
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
            .map_err(|e| format!("Error with response: {:?}", e))?;

        Ok(response.into_inner())
    }

    pub async fn get_current_zec_price(uri: http::Uri) -> Result<PriceResponse, String> {
        let client = Arc::new(GrpcConnector::new(uri));
        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;
        let request = Request::new(Empty {});

        let response = client
            .get_current_zec_price(request)
            .await
            .map_err(|e| format!("Error with response: {:?}", e))?;

        Ok(response.into_inner())
    }

    pub async fn get_historical_zec_prices(
        uri: http::Uri,
        transaction_ids: Vec<(TxId, u64)>,
        currency: String,
    ) -> Result<HashMap<TxId, Option<f64>>, String> {
        let client = Arc::new(GrpcConnector::new(uri));
        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let mut prices = HashMap::new();
        let mut error_count: u32 = 0;

        for (transaction_id, ts) in transaction_ids {
            if error_count < 10 {
                let r = Request::new(PriceRequest {
                    timestamp: ts,
                    currency: currency.clone(),
                });
                match client.get_zec_price(r).await {
                    Ok(response) => {
                        let price_response = response.into_inner();
                        prices.insert(transaction_id, Some(price_response.price));
                    }
                    Err(e) => {
                        // If the server doesn't support this, bail
                        if e.code() == tonic::Code::Unimplemented {
                            return Err(format!("Unsupported by server"));
                        }

                        // Ignore other errors, these are probably just for the particular date/time
                        // and will be retried anyway
                        warn!("Ignoring grpc error: {}", e);
                        error_count += 1;
                        prices.insert(transaction_id, None);
                    }
                }
            } else {
                // If there are too many errors, don't bother querying the server, just return none
                prices.insert(transaction_id, None);
            }
        }

        Ok(prices)
    }

    // get_latest_block GRPC call
    pub async fn get_latest_block(uri: http::Uri) -> Result<BlockId, String> {
        let client = Arc::new(GrpcConnector::new(uri));
        let mut client = client
            .get_client()
            .await
            .map_err(|e| format!("Error getting client: {:?}", e))?;

        let request = Request::new(ChainSpec {});

        let response = client
            .get_latest_block(request)
            .await
            .map_err(|e| format!("Error with response: {:?}", e))?;

        Ok(response.into_inner())
    }

    pub async fn send_transaction(uri: http::Uri, transaction_bytes: Box<[u8]>) -> Result<String, String> {
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
async fn add_tls_test_config(tls: ClientTlsConfig) -> ClientTlsConfig {
    let (cert, identity) = crate::lightclient::test_server::get_tls_test_pem();
    tls.ca_certificate(cert).identity(identity)
}

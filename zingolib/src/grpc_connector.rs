//! TODO: Add Mod Description Here!

use std::sync::Arc;

use futures::future::join_all;
use futures::stream::FuturesUnordered;
use futures::StreamExt;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tonic::Request;
use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_client_backend::proto::service::{
    BlockId, BlockRange, ChainSpec, Empty, LightdInfo, RawTransaction,
    TransparentAddressBlockFilter, TreeState, TxFilter,
};
use zcash_primitives::consensus::{BlockHeight, BranchId, Parameters};
use zcash_primitives::transaction::{Transaction, TxId};
pub(crate) use zingo_netutils::GrpcConnector;

/// TODO: Add Doc Comment Here!
pub async fn start_saplingtree_fetcher(
    conn: &GrpcConnector,
) -> (
    tokio::task::JoinHandle<()>,
    UnboundedSender<(u64, oneshot::Sender<Result<TreeState, String>>)>,
) {
    let (transmitter, mut receiver) =
        unbounded_channel::<(u64, oneshot::Sender<Result<TreeState, String>>)>();
    let uri = conn.uri().clone();

    let h = tokio::spawn(async move {
        let uri = uri.clone();
        while let Some((height, result_transmitter)) = receiver.recv().await {
            result_transmitter
                .send(get_trees(uri.clone(), height).await)
                .unwrap()
        }
    });

    (h, transmitter)
}

/// TODO: Add Doc Comment Here!
pub async fn start_taddr_transaction_fetcher(
    conn: &GrpcConnector,
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
    let uri = conn.uri().clone();

    let h = tokio::spawn(async move {
        let uri = uri.clone();
        if let Ok(((taddrs, start_height, end_height), result_transmitter)) = receiver.await {
            let mut transaction_receivers = vec![];
            let mut transaction_receivers_workers = vec![];

            // Create a stream for every t-addr
            for taddr in taddrs {
                let (transaction_s, transaction_receiver) = unbounded_channel();
                transaction_receivers.push(transaction_receiver);
                transaction_receivers_workers.push(tokio::spawn(get_taddr_transactions(
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

/// TODO: Add Doc Comment Here!
pub async fn start_full_transaction_fetcher(
    conn: &GrpcConnector,
    network: impl Parameters + Send + Copy + 'static,
) -> (
    JoinHandle<()>,
    UnboundedSender<(TxId, oneshot::Sender<Result<Transaction, String>>)>,
) {
    let (transmitter, mut receiver) =
        unbounded_channel::<(TxId, oneshot::Sender<Result<Transaction, String>>)>();
    let uri = conn.uri().clone();

    let h = tokio::spawn(async move {
        let mut workers = FuturesUnordered::new();
        while let Some((transaction_id, result_transmitter)) = receiver.recv().await {
            let uri = uri.clone();
            workers.push(tokio::spawn(async move {
                result_transmitter
                    .send(get_full_transaction(uri.clone(), &transaction_id, network).await)
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

/// TODO: Add Doc Comment Here!
pub async fn get_block_range(
    conn: &GrpcConnector,
    start_height: u64,
    end_height: u64,
    senders: &[UnboundedSender<CompactBlock>; 2],
) -> Result<(), String> {
    let mut client = conn.get_client().await.map_err(|e| format!("{}", e))?;

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

/// TODO: Add Doc Comment Here!
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

/// TODO: Add Doc Comment Here!
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

/// TODO: Add Doc Comment Here!
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

/// TODO: Add Doc Comment Here!
pub async fn get_trees(uri: http::Uri, height: u64) -> Result<TreeState, String> {
    let client = Arc::new(GrpcConnector::new(uri.clone()));
    let mut client = client
        .get_client()
        .await
        .map_err(|e| format!("Error getting client: {:?}", e))?;

    let b = BlockId {
        height,
        hash: vec![],
    };
    let response = client
        .get_tree_state(Request::new(b))
        .await
        .map_err(|e| format!("Error with get_tree_state response at {uri}: {:?}", e))?;

    Ok(response.into_inner())
}

/// get_latest_block GRPC call
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

/// TODO: Add Doc Comment Here!
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
        if transaction_id.starts_with('\"') && transaction_id.ends_with('\"') {
            transaction_id = transaction_id[1..transaction_id.len() - 1].to_string();
        }

        Ok(transaction_id)
    } else {
        Err(format!("Error: {:?}", sendresponse))
    }
}

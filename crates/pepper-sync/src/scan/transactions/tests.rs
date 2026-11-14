use zcash_protocol::consensus::MAIN_NETWORK;

use super::*;

// A fetcher that answers every transaction request with `status`.
fn fetcher_answering(status: tonic::Status) -> mpsc::UnboundedSender<FetchRequest> {
    let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(fetch_request) = fetch_request_receiver.recv().await {
            match fetch_request {
                FetchRequest::Transaction(reply_sender, _) => {
                    let _ignore_error = reply_sender.send(Err(status.clone()));
                }
                _ => panic!("unexpected fetch request"),
            }
        }
    });
    fetch_request_sender
}

async fn scan(scan_target: ScanTarget) -> Result<ScannedTransactions, ScanError> {
    scan_against(scan_target, tonic::Status::not_found("no such transaction")).await
}

async fn scan_against(
    scan_target: ScanTarget,
    status: tonic::Status,
) -> Result<ScannedTransactions, ScanError> {
    scan_transactions(
        fetcher_answering(status),
        &MAIN_NETWORK,
        &HashMap::new(),
        BTreeSet::from([scan_target]),
        DecryptedNoteData::new(),
        &BTreeMap::new(),
        &mut BTreeMap::new(),
        HashMap::new(),
    )
    .await
}

#[tokio::test]
async fn an_external_target_the_server_does_not_serve_is_dropped() {
    let scan_target = ScanTarget::new(BlockHeight::from_u32(100), Some(TxId::from_bytes([1; 32])));

    let scanned = scan(scan_target).await.unwrap();

    assert!(scanned.transactions.is_empty());
    assert_eq!(scanned.dropped_targets, BTreeSet::from([scan_target]));
}

// The transaction came from the served data, so the server not serving it is an inconsistency in that
// data, reported with the recommendation to change server.
#[tokio::test]
async fn an_internal_target_the_server_does_not_serve_is_an_inconsistency() {
    let txid = TxId::from_bytes([1; 32]);
    let scan_target = ScanTarget::internal(BlockHeight::from_u32(100), txid);

    assert!(matches!(
        scan(scan_target).await,
        Err(ScanError::TransactionNotServed { txid: unserved, block_height, status })
            if unserved == txid
                && block_height == BlockHeight::from_u32(100)
                && status.code() == tonic::Code::NotFound
    ));
}

// Network weather is not an answer about the transaction: the request failure is reported as the transient
// failure it is, and a consumer's target is not dropped for it.
#[tokio::test]
async fn network_weather_is_reported_and_drops_no_target() {
    for scan_target in [
        ScanTarget::new(BlockHeight::from_u32(100), Some(TxId::from_bytes([1; 32]))),
        ScanTarget::internal(BlockHeight::from_u32(100), TxId::from_bytes([1; 32])),
    ] {
        assert!(matches!(
            scan_against(scan_target, tonic::Status::unavailable("down")).await,
            Err(ScanError::ServerError(ServerError::RequestFailed(status)))
                if status.code() == tonic::Code::Unavailable
        ));
    }
}

#[tokio::test]
async fn a_target_without_a_txid_fetches_no_transaction() {
    let scanned = scan(ScanTarget::new(BlockHeight::from_u32(100), None))
        .await
        .unwrap();

    assert!(scanned.transactions.is_empty());
    assert!(scanned.dropped_targets.is_empty());
}

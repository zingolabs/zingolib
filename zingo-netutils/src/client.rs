//! Module for structs and functions associated with light-clients

use crate::{GetClientError, GrpcConnector};
use http_body_util::combinators::UnsyncBoxBody;
use prost;
use tower::util::BoxCloneService;

/// The underlying service type used for gRPC connections
pub type UnderlyingService = BoxCloneService<
    http::Request<UnsyncBoxBody<prost::bytes::Bytes, tonic::Status>>,
    http::Response<hyper::body::Incoming>,
    hyper_util::client::legacy::Error,
>;

/// Builds a client for creating RPC requests to the indexer/light-node
pub async fn build_client(
    uri: http::Uri,
) -> Result<CompactTxStreamerClient<UnderlyingService>, GetClientError> {
    GrpcConnector::new(uri).get_client().await
}

/// ?
use http_body::Body;
use hyper_util::client::legacy::{Client, connect::Connect};
use zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient;
/// a utility used in multiple places
pub fn client_from_connector<C, B>(connector: C, http2_only: bool) -> Box<Client<C, B>>
where
    C: Connect + Clone,
    B: Body + Send,
    B::Data: Send,
{
    Box::new(
        Client::builder(hyper_util::rt::TokioExecutor::new())
            .http2_only(http2_only)
            .build(connector),
    )
}

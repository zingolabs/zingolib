//! Tower service type definitions shared between crates

use http_body_util::combinators::UnsyncBoxBody;
use tonic::Status;
use tower::util::BoxCloneService;

/// The underlying service type used for gRPC connections
pub type UnderlyingService = BoxCloneService<
    http::Request<UnsyncBoxBody<prost::bytes::Bytes, Status>>,
    http::Response<hyper::body::Incoming>,
    hyper_util::client::legacy::Error,
>;
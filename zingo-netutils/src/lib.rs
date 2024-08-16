//! Zingo-Netutils
//!
//! This crate provides the `GrpcConnector` struct,
//! used to communicate with a lightwalletd

#![warn(missing_docs)]
use std::sync::Arc;
use tower::ServiceExt;

use http::{uri::PathAndQuery, Uri};
use http_body::combinators::UnsyncBoxBody;
use hyper::client::HttpConnector;
use thiserror::Error;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tonic::Status;
use tower::util::BoxCloneService;
use zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient;

/// TODO: add doc-comment
pub type UnderlyingService = BoxCloneService<
    http::Request<UnsyncBoxBody<prost::bytes::Bytes, Status>>,
    http::Response<hyper::Body>,
    hyper::Error,
>;

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, Error)]
pub enum GetClientError {
    #[error("bad uri: invalid scheme")]
    InvalidScheme,
    #[error("bad uri: invalid authority")]
    InvalidAuthority,
    #[error("bad uri: invalid path and/or query")]
    InvalidPathAndQuery,
}

/// The connector, containing the URI to connect to.
/// This type is mostly an interface to the get_client method,
/// the proto-generated CompactTxStreamerClient type is the main
/// interface to actually communicating with a lightwalletd.
#[derive(Clone)]
pub struct GrpcConnector {
    uri: http::Uri,
}

impl GrpcConnector {
    /// Takes a URI, and wraps in a GrpcConnector
    pub fn new(uri: http::Uri) -> Self {
        Self { uri }
    }

    /// The URI to connect to
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    /// Connect to the URI, and return a Client. For the full list of methods
    /// the client supports, see the service.proto file (some of the types
    /// are defined in the compact_formats.proto file)
    pub fn get_client(
        &self,
    ) -> impl std::future::Future<
        Output = Result<CompactTxStreamerClient<UnderlyingService>, GetClientError>,
    > {
        let uri = Arc::new(self.uri.clone());
        async move {
            let mut http_connector = HttpConnector::new();
            http_connector.enforce_http(false);
            let scheme = uri.scheme().ok_or(GetClientError::InvalidScheme)?.clone();
            let authority = uri
                .authority()
                .ok_or(GetClientError::InvalidAuthority)?
                .clone();
            if uri.scheme_str() == Some("https") {
                let mut roots = RootCertStore {
                    roots: webpki_roots::TLS_SERVER_ROOTS
                        .0
                        .iter()
                        .map(|anchor| rustls_pki_types::TrustAnchor {
                            subject: rustls_pki_types::Der::from_slice(anchor.subject),
                            subject_public_key_info: rustls_pki_types::Der::from_slice(anchor.spki),
                            name_constraints: anchor
                                .name_constraints
                                .map(|o| rustls_pki_types::Der::from_slice(o)),
                        })
                        .collect(),
                };

                #[cfg(test)]
                add_test_cert_to_roots(&mut roots);

                let tls = ClientConfig::builder()
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
                let svc = tower::ServiceBuilder::new()
                    //Here, we take all the pieces of our uri, and add in the path from the Requests's uri
                    .map_request(move |mut request: http::Request<tonic::body::BoxBody>| {
                        let path_and_query = request
                            .uri()
                            .path_and_query()
                            .cloned()
                            .unwrap_or(PathAndQuery::from_static("/"));
                        let uri = Uri::builder()
                            .scheme(scheme.clone())
                            .authority(authority.clone())
                            //here. The Request's uri contains the path to the GRPC sever and
                            //the method being called
                            .path_and_query(path_and_query)
                            .build()
                            .unwrap();

                        *request.uri_mut() = uri;
                        request
                    })
                    .service(client);

                Ok(CompactTxStreamerClient::new(svc.boxed_clone()))
            } else {
                let connector = tower::ServiceBuilder::new().service(http_connector);
                let client = Box::new(hyper::Client::builder().http2_only(true).build(connector));
                let svc = tower::ServiceBuilder::new()
                    //Here, we take all the pieces of our uri, and add in the path from the Requests's uri
                    .map_request(move |mut request: http::Request<tonic::body::BoxBody>| {
                        let path_and_query = request
                            .uri()
                            .path_and_query()
                            .cloned()
                            .unwrap_or(PathAndQuery::from_static("/"));
                        let uri = Uri::builder()
                            .scheme(scheme.clone())
                            .authority(authority.clone())
                            //here. The Request's uri contains the path to the GRPC sever and
                            //the method being called
                            .path_and_query(path_and_query)
                            .build()
                            .unwrap();

                        *request.uri_mut() = uri;
                        request
                    })
                    .service(client);

                Ok(CompactTxStreamerClient::new(svc.boxed_clone()))
            }
        }
    }
}

#[cfg(test)]
fn add_test_cert_to_roots(roots: &mut RootCertStore) {
    use rustls_pki_types::CertificateDer;

    const TEST_PEMFILE_PATH: &str = "test-data/localhost.pem";
    let fd = std::fs::File::open(TEST_PEMFILE_PATH).unwrap();
    let mut buf = std::io::BufReader::new(&fd);
    let certs = rustls_pemfile::certs(&mut buf).unwrap();
    roots.add_parsable_certificates(certs.iter().map(|cert| CertificateDer::from_slice(cert)));
}

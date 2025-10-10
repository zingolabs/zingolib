//! Module for creating GRPC clients compatible with `zcash_client_backend`

use http::{Uri, uri::PathAndQuery};
use hyper_util::client::legacy::connect::HttpConnector;
use std::sync::Arc;
use tokio_rustls::rustls::pki_types::{Der, TrustAnchor};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tower::ServiceExt;
use zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient;
use zingo_netutils::UnderlyingService;

/// Creates a `zcash_client_backend` compatible GRPC client from a URI
/// This duplicates the connection logic from `zingo_netutils` but creates a `zcash_client_backend` client
pub async fn get_zcb_client(
    uri: Uri,
) -> Result<CompactTxStreamerClient<UnderlyingService>, zingo_netutils::GetClientError> {
    let uri = Arc::new(uri);
    let mut http_connector = HttpConnector::new();
    http_connector.enforce_http(false);
    let scheme = uri
        .scheme()
        .ok_or(zingo_netutils::GetClientError::InvalidScheme)?
        .clone();
    let authority = uri
        .authority()
        .ok_or(zingo_netutils::GetClientError::InvalidAuthority)?
        .clone();

    if uri.scheme_str() == Some("https") {
        let mut root_store = RootCertStore::empty();
        root_store.extend(
            webpki_roots::TLS_SERVER_ROOTS
                .iter()
                .map(|anchor_ref| TrustAnchor {
                    subject: Der::from_slice(anchor_ref.subject),
                    subject_public_key_info: Der::from_slice(anchor_ref.spki),
                    name_constraints: anchor_ref.name_constraints.map(Der::from_slice),
                }),
        );

        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let connector = tower::ServiceBuilder::new()
            .layer_fn(move |s| {
                let tls = config.clone();

                hyper_rustls::HttpsConnectorBuilder::new()
                    .with_tls_config(tls)
                    .https_or_http()
                    .enable_http2()
                    .wrap_connector(s)
            })
            .service(http_connector);

        let client = zingo_netutils::client::client_from_connector(connector, false);
        let svc = tower::ServiceBuilder::new()
            .map_request(move |mut request: http::Request<_>| {
                let path_and_query = request
                    .uri()
                    .path_and_query()
                    .cloned()
                    .unwrap_or(PathAndQuery::from_static("/"));
                let uri = Uri::builder()
                    .scheme(scheme.clone())
                    .authority(authority.clone())
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
        let client = zingo_netutils::client::client_from_connector(connector, true);
        let svc = tower::ServiceBuilder::new()
            .map_request(move |mut request: http::Request<_>| {
                let path_and_query = request
                    .uri()
                    .path_and_query()
                    .cloned()
                    .unwrap_or(PathAndQuery::from_static("/"));
                let uri = Uri::builder()
                    .scheme(scheme.clone())
                    .authority(authority.clone())
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

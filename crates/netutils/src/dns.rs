#![forbid(unsafe_code)]

use std::future::Future;
use std::io;
use std::net;
use std::pin;
use std::sync;
use std::task;
use std::vec;

use hickory_resolver::config;
use hickory_resolver::net as resolver_net;
use hickory_resolver::net::runtime;
use hyper_util::client::legacy::connect::dns as hyper_dns;

const UPSTREAMS: [config::ServerGroup<'static>; 2] = [config::CLOUDFLARE, config::QUAD9];

const URI_PORT: u16 = 0;

static RESOLVER: sync::OnceLock<Result<hickory_resolver::TokioResolver, resolver_net::NetError>> =
    sync::OnceLock::new();

fn resolver_config() -> config::ResolverConfig {
    config::ResolverConfig::from_name_servers(
        UPSTREAMS
            .iter()
            .flat_map(|group| group.https().chain(group.tls()))
            .collect(),
    )
}

/// - reads the system hosts file
fn build_resolver(
    config: config::ResolverConfig,
    tls: rustls::ClientConfig,
) -> Result<hickory_resolver::TokioResolver, resolver_net::NetError> {
    hickory_resolver::TokioResolver::builder_with_config(
        config,
        runtime::TokioRuntimeProvider::default(),
    )
    .with_tls_config(tls)
    .build()
}

/// - first call: installs the process crypto provider, builds the shared resolver
fn shared_resolver() -> Result<&'static hickory_resolver::TokioResolver, resolver_net::NetError> {
    RESOLVER
        .get_or_init(|| {
            build_resolver(
                resolver_config(),
                crate::client_tls_config(crate::mozilla_roots()),
            )
        })
        .as_ref()
        .map_err(resolver_net::NetError::clone)
}

fn io_error(error: resolver_net::NetError) -> io::Error {
    let kind = match error {
        resolver_net::NetError::Timeout => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}

/// - network: DoH/DoT query on cache miss
async fn lookup_with(
    resolver: &hickory_resolver::TokioResolver,
    host: &str,
) -> Result<Vec<net::IpAddr>, resolver_net::NetError> {
    Ok(resolver.lookup_ip(host).await?.iter().collect())
}

/// - first call: [`shared_resolver`]
/// - network: [`lookup_with`]
pub async fn lookup(host: &str) -> io::Result<Vec<net::IpAddr>> {
    let resolver = shared_resolver().map_err(io_error)?;
    lookup_with(resolver, host).await.map_err(io_error)
}

fn uri_addrs(ips: Vec<net::IpAddr>) -> vec::IntoIter<net::SocketAddr> {
    ips.into_iter()
        .map(|ip| net::SocketAddr::new(ip, URI_PORT))
        .collect::<Vec<_>>()
        .into_iter()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EncryptedResolver;

impl tower::Service<hyper_dns::Name> for EncryptedResolver {
    type Response = vec::IntoIter<net::SocketAddr>;
    type Error = io::Error;
    type Future = pin::Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut task::Context<'_>) -> task::Poll<io::Result<()>> {
        task::Poll::Ready(Ok(()))
    }

    /// - network: [`lookup`]
    fn call(&mut self, name: hyper_dns::Name) -> Self::Future {
        Box::pin(async move { lookup(name.as_str()).await.map(uri_addrs) })
    }
}

#[cfg(feature = "socks5-fetch")]
impl reqwest::dns::Resolve for EncryptedResolver {
    /// - network: [`lookup`]
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: reqwest::dns::Addrs = Box::new(uri_addrs(lookup(name.as_str()).await?));
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::default_indexer;

    #[test]
    fn every_upstream_is_encrypted() {
        let plaintext_upstreams: Vec<_> = resolver_config()
            .name_servers()
            .iter()
            .flat_map(|server| &server.connections)
            .map(|connection| connection.protocol.to_protocol())
            .filter(|protocol| !protocol.is_encrypted())
            .collect();
        assert!(
            plaintext_upstreams.is_empty(),
            "upstreams must be DoH/DoT only, found {plaintext_upstreams:?}"
        );
    }

    #[test]
    fn uri_addrs_carry_the_placeholder_port() {
        let ips = vec![
            net::Ipv4Addr::LOCALHOST.into(),
            net::Ipv6Addr::LOCALHOST.into(),
        ];
        assert_eq!(
            uri_addrs(ips).collect::<Vec<_>>(),
            vec![
                net::SocketAddr::new(net::Ipv4Addr::LOCALHOST.into(), URI_PORT),
                net::SocketAddr::new(net::Ipv6Addr::LOCALHOST.into(), URI_PORT),
            ],
        );
    }

    #[tokio::test]
    async fn every_answer_is_local_or_encrypted() {
        let indexer: http::Uri = default_indexer::MAINNET_DEFAULT_INDEXER
            .parse()
            .expect("MAINNET_DEFAULT_INDEXER is a valid URI literal");
        let host = indexer
            .host()
            .expect("MAINNET_DEFAULT_INDEXER names a host");

        let v4_literal = lookup(&net::Ipv4Addr::LOCALHOST.to_string()).await;
        let v6_literal = lookup(&net::Ipv6Addr::LOCALHOST.to_string()).await;
        let localhost = lookup("localhost").await;
        assert_eq!(
            (
                v4_literal.expect("an IPv4 literal answers without a query"),
                v6_literal.expect("an IPv6 literal answers without a query"),
            ),
            (
                vec![net::IpAddr::from(net::Ipv4Addr::LOCALHOST)],
                vec![net::IpAddr::from(net::Ipv6Addr::LOCALHOST)],
            ),
        );
        assert!(
            localhost
                .as_ref()
                .is_ok_and(|ips| ips.contains(&net::Ipv4Addr::LOCALHOST.into())),
            "localhost must answer loopback locally, got {localhost:?}"
        );

        let public = lookup(host)
            .await
            .expect("the mainnet Default Indexer host has a public A/AAAA record");
        assert!(
            !public.is_empty(),
            "{host} must resolve over DoH/DoT, got {public:?}"
        );
    }
}

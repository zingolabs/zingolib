//! - DoH/DoT only (Cloudflare + Quad9 by IP, no system resolver, no plaintext fallback)
//! - IP literals, `localhost` & hosts file answered locally

#![forbid(unsafe_code)]

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use hickory_resolver::TokioResolver;
use hickory_resolver::config::{CLOUDFLARE, QUAD9, ResolverConfig, ServerGroup};
use hickory_resolver::net::NetError;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hyper_util::client::legacy::connect::dns::Name;

use crate::{client_tls_config, mozilla_roots};

const UPSTREAMS: [ServerGroup<'static>; 2] = [CLOUDFLARE, QUAD9];

// placeholder port (hyper & reqwest overwrite w/ URI port)
const URI_PORT: u16 = 0;

// process-wide (shared answer cache & DoH/DoT connections)
static RESOLVER: OnceLock<Result<TokioResolver, NetError>> = OnceLock::new();

fn resolver_config() -> ResolverConfig {
    ResolverConfig::from_name_servers(
        UPSTREAMS
            .iter()
            .flat_map(|group| group.https().chain(group.tls()))
            .collect(),
    )
}

/// - reads the system hosts file
fn build_resolver(
    config: ResolverConfig,
    tls: rustls::ClientConfig,
) -> Result<TokioResolver, NetError> {
    TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
        .with_tls_config(tls)
        .build()
}

/// - first call: installs the process crypto provider, builds the shared resolver
fn shared_resolver() -> Result<&'static TokioResolver, NetError> {
    RESOLVER
        .get_or_init(|| build_resolver(resolver_config(), client_tls_config(mozilla_roots())))
        .as_ref()
        .map_err(NetError::clone)
}

fn io_error(error: NetError) -> io::Error {
    let kind = match error {
        NetError::Timeout => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}

/// - network: DoH/DoT query on cache miss
async fn lookup_with(resolver: &TokioResolver, host: &str) -> Result<Vec<IpAddr>, NetError> {
    Ok(resolver.lookup_ip(host).await?.iter().collect())
}

/// - first call: [`shared_resolver`]
/// - network: [`lookup_with`]
pub async fn lookup(host: &str) -> io::Result<Vec<IpAddr>> {
    let resolver = shared_resolver().map_err(io_error)?;
    lookup_with(resolver, host).await.map_err(io_error)
}

fn uri_addrs(ips: Vec<IpAddr>) -> std::vec::IntoIter<SocketAddr> {
    ips.into_iter()
        .map(|ip| SocketAddr::new(ip, URI_PORT))
        .collect::<Vec<_>>()
        .into_iter()
}

// [`lookup`] as hyper's & reqwest's resolver (in place of their `getaddrinfo` default)
#[derive(Clone, Copy, Debug, Default)]
pub struct EncryptedResolver;

impl tower::Service<Name> for EncryptedResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<Self::Response>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    /// - network: [`lookup`]
    fn call(&mut self, name: Name) -> Self::Future {
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
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;
    use crate::default_indexer::MAINNET_DEFAULT_INDEXER;

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
        let ips = vec![Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()];
        assert_eq!(
            uri_addrs(ips).collect::<Vec<_>>(),
            vec![
                SocketAddr::new(Ipv4Addr::LOCALHOST.into(), URI_PORT),
                SocketAddr::new(Ipv6Addr::LOCALHOST.into(), URI_PORT),
            ],
        );
    }

    #[tokio::test]
    async fn every_answer_is_local_or_encrypted() {
        let indexer: http::Uri = MAINNET_DEFAULT_INDEXER
            .parse()
            .expect("MAINNET_DEFAULT_INDEXER is a valid URI literal");
        let host = indexer
            .host()
            .expect("MAINNET_DEFAULT_INDEXER names a host");

        let v4_literal = lookup(&Ipv4Addr::LOCALHOST.to_string()).await;
        let v6_literal = lookup(&Ipv6Addr::LOCALHOST.to_string()).await;
        let localhost = lookup("localhost").await;
        assert_eq!(
            (
                v4_literal.expect("an IPv4 literal answers without a query"),
                v6_literal.expect("an IPv6 literal answers without a query"),
            ),
            (
                vec![IpAddr::from(Ipv4Addr::LOCALHOST)],
                vec![IpAddr::from(Ipv6Addr::LOCALHOST)],
            ),
        );
        assert!(
            localhost
                .as_ref()
                .is_ok_and(|ips| ips.contains(&Ipv4Addr::LOCALHOST.into())),
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

//! - DoH/DoT only (Cloudflare + Quad9 by IP, no system resolver, no plaintext fallback)
//! - IP literals, `localhost` & hosts file answered locally

#![forbid(unsafe_code)]

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use hickory_resolver::TokioResolver;
use hickory_resolver::config::{CLOUDFLARE, NameServerConfig, QUAD9, ResolverConfig, ServerGroup};
use hickory_resolver::net::NetError;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hyper_util::client::legacy::connect::dns::Name;

use crate::{client_tls_config, mozilla_roots};

const UPSTREAMS: [ServerGroup<'static>; 2] = [CLOUDFLARE, QUAD9];

// placeholder port (hyper & reqwest overwrite w/ URI port)
const URI_PORT: u16 = 0;

// process-wide (shared answer cache & DoH/DoT connections)
static RESOLVER: OnceLock<TokioResolver> = OnceLock::new();

fn name_servers() -> Vec<NameServerConfig> {
    UPSTREAMS
        .iter()
        .flat_map(|group| group.https().chain(group.tls()))
        .collect()
}

fn resolver() -> Result<&'static TokioResolver, NetError> {
    if let Some(built) = RESOLVER.get() {
        return Ok(built);
    }
    let built = TokioResolver::builder_with_config(
        ResolverConfig::from_name_servers(name_servers()),
        TokioRuntimeProvider::default(),
    )
    .with_tls_config(client_tls_config(mozilla_roots()))
    .build()?;
    Ok(RESOLVER.get_or_init(|| built))
}

fn io_error(error: NetError) -> io::Error {
    let kind = match error {
        NetError::Timeout => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}

/// - first call: installs the process crypto provider, builds the shared resolver
/// - network: DoH/DoT query on cache miss
pub async fn lookup(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    let ips = resolver()
        .map_err(io_error)?
        .lookup_ip(host)
        .await
        .map_err(io_error)?;
    Ok(ips.iter().map(|ip| SocketAddr::new(ip, port)).collect())
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
        Box::pin(async move { lookup(name.as_str(), URI_PORT).await.map(Vec::into_iter) })
    }
}

#[cfg(feature = "socks5-fetch")]
impl reqwest::dns::Resolve for EncryptedResolver {
    /// - network: [`lookup`]
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs = lookup(name.as_str(), URI_PORT).await?;
            let addrs: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;
    use crate::default_indexer::MAINNET_DEFAULT_INDEXER;

    #[tokio::test]
    async fn every_answer_is_local_or_encrypted() {
        let plaintext_upstreams: Vec<_> = name_servers()
            .into_iter()
            .flat_map(|server| server.connections)
            .map(|connection| connection.protocol.to_protocol())
            .filter(|protocol| !protocol.is_encrypted())
            .collect();
        assert!(
            plaintext_upstreams.is_empty(),
            "upstreams must be DoH/DoT only, found {plaintext_upstreams:?}"
        );

        let indexer: http::Uri = MAINNET_DEFAULT_INDEXER
            .parse()
            .expect("MAINNET_DEFAULT_INDEXER is a valid URI literal");
        let host = indexer
            .host()
            .expect("MAINNET_DEFAULT_INDEXER names a host");
        let port = indexer
            .port_u16()
            .expect("MAINNET_DEFAULT_INDEXER names its port");
        let at_port = |ip: IpAddr| SocketAddr::new(ip, port);

        let v4_literal = lookup(&Ipv4Addr::LOCALHOST.to_string(), port).await;
        let v6_literal = lookup(&Ipv6Addr::LOCALHOST.to_string(), port).await;
        let localhost = lookup("localhost", port).await;
        assert_eq!(
            (
                v4_literal.expect("an IPv4 literal answers without a query"),
                v6_literal.expect("an IPv6 literal answers without a query"),
            ),
            (
                vec![at_port(Ipv4Addr::LOCALHOST.into())],
                vec![at_port(Ipv6Addr::LOCALHOST.into())],
            ),
        );
        assert!(
            localhost
                .as_ref()
                .is_ok_and(|addrs| addrs.contains(&at_port(Ipv4Addr::LOCALHOST.into()))),
            "localhost must answer loopback locally, got {localhost:?}"
        );

        let public = lookup(host, port)
            .await
            .expect("the mainnet Default Indexer host has a public A/AAAA record");
        assert!(
            !public.is_empty() && public.iter().all(|addr| addr.port() == port),
            "{host} must resolve over DoH/DoT to addresses at port {port}, got {public:?}"
        );
    }
}

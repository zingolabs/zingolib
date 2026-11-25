#![forbid(unsafe_code)]

use std::io;
use std::net;
use std::sync;
use std::vec;

use hickory_resolver::config;
use hickory_resolver::net as resolver_net;
use hickory_resolver::net::runtime;
use hyper_util::client::legacy::connect::dns as hyper_dns;
use tower::ServiceExt as _;

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resolver {
    #[default]
    Encrypted,
    System,
}

#[cfg(test)]
pub(crate) fn encrypted_resolver_is_built() -> bool {
    RESOLVER.get().is_some()
}

fn ips(addrs: impl Iterator<Item = net::SocketAddr>) -> Vec<net::IpAddr> {
    addrs.map(|addr| addr.ip()).collect()
}

fn hyper_name(host: &str) -> io::Result<hyper_dns::Name> {
    host.parse()
        .map_err(|invalid| io::Error::new(io::ErrorKind::InvalidInput, invalid))
}

/// - first call: installs the process crypto provider, builds the shared resolver
/// - network: DoH/DoT query on cache miss
async fn encrypted_lookup(host: &str) -> io::Result<Vec<net::IpAddr>> {
    let resolver = shared_resolver().map_err(io_error)?;
    lookup_with(resolver, host).await.map_err(io_error)
}

async fn system_lookup(host: &str) -> io::Result<Vec<net::IpAddr>> {
    hyper_dns::GaiResolver::new()
        .oneshot(hyper_name(host)?)
        .await
        .map(ips)
}

pub async fn lookup_through(resolver: Resolver, host: &str) -> io::Result<Vec<net::IpAddr>> {
    match resolver {
        Resolver::Encrypted => encrypted_lookup(host).await,
        Resolver::System => system_lookup(host).await,
    }
}

pub async fn lookup(host: &str) -> io::Result<Vec<net::IpAddr>> {
    lookup_through(Resolver::Encrypted, host).await
}

fn uri_addrs(ips: Vec<net::IpAddr>) -> vec::IntoIter<net::SocketAddr> {
    ips.into_iter()
        .map(|ip| net::SocketAddr::new(ip, URI_PORT))
        .collect::<Vec<_>>()
        .into_iter()
}

async fn lookup_uri_addrs(
    resolver: Resolver,
    name: hyper_dns::Name,
) -> io::Result<vec::IntoIter<net::SocketAddr>> {
    lookup_through(resolver, name.as_str()).await.map(uri_addrs)
}

pub fn hyper_resolver(
    resolver: Resolver,
) -> impl tower::Service<
    hyper_dns::Name,
    Response = vec::IntoIter<net::SocketAddr>,
    Error = io::Error,
    Future: Send,
> + Clone
+ Send
+ Sync
+ 'static {
    tower::service_fn(move |name| lookup_uri_addrs(resolver, name))
}

#[cfg(feature = "socks5-fetch")]
#[derive(Clone, Copy, Debug, Default)]
pub struct EncryptedResolver;

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
    fn the_default_resolver_is_encrypted() {
        assert_eq!(Resolver::default(), Resolver::Encrypted);
    }

    #[test]
    fn ips_drop_the_port_and_keep_the_order() {
        let addrs = [
            net::SocketAddr::new(net::Ipv6Addr::LOCALHOST.into(), URI_PORT),
            net::SocketAddr::new(net::Ipv4Addr::LOCALHOST.into(), URI_PORT),
        ];
        assert_eq!(
            ips(addrs.into_iter()),
            vec![
                net::IpAddr::from(net::Ipv6Addr::LOCALHOST),
                net::IpAddr::from(net::Ipv4Addr::LOCALHOST),
            ],
        );
    }

    #[tokio::test]
    async fn a_system_lookup_answers_and_never_builds_the_encrypted_resolver() -> io::Result<()> {
        let v4_literal =
            lookup_through(Resolver::System, &net::Ipv4Addr::LOCALHOST.to_string()).await?;
        let v6_literal =
            lookup_through(Resolver::System, &net::Ipv6Addr::LOCALHOST.to_string()).await?;
        let localhost = lookup_through(Resolver::System, "localhost").await?;

        assert_eq!(
            (v4_literal, v6_literal),
            (
                vec![net::IpAddr::from(net::Ipv4Addr::LOCALHOST)],
                vec![net::IpAddr::from(net::Ipv6Addr::LOCALHOST)],
            ),
        );
        assert!(
            localhost.iter().any(net::IpAddr::is_loopback),
            "localhost must answer a loopback address, got {localhost:?}"
        );
        assert!(!encrypted_resolver_is_built());
        Ok(())
    }

    #[tokio::test]
    async fn an_encrypted_lookup_builds_the_encrypted_resolver() -> io::Result<()> {
        lookup_through(Resolver::Encrypted, &net::Ipv4Addr::LOCALHOST.to_string()).await?;
        assert!(encrypted_resolver_is_built());
        Ok(())
    }

    #[tokio::test]
    async fn the_system_hyper_resolver_answers_at_the_placeholder_port() -> io::Result<()> {
        let name = hyper_name(&net::Ipv4Addr::LOCALHOST.to_string())?;
        let addrs = hyper_resolver(Resolver::System).oneshot(name).await?;
        assert_eq!(
            addrs.collect::<Vec<_>>(),
            vec![net::SocketAddr::new(
                net::Ipv4Addr::LOCALHOST.into(),
                URI_PORT
            )],
        );
        assert!(!encrypted_resolver_is_built());
        Ok(())
    }

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

    fn host_of(indexer: &str) -> String {
        indexer
            .parse::<http::Uri>()
            .expect("a Default Indexer constant is a valid URI literal")
            .host()
            .expect("a Default Indexer constant names a host")
            .to_string()
    }

    fn new_runtime() -> io::Result<tokio::runtime::Runtime> {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
    }

    #[test]
    #[ignore = "resolves live host names through Cloudflare and Quad9"]
    fn a_lookup_survives_the_runtime_that_built_the_resolver() -> io::Result<()> {
        let mainnet = host_of(default_indexer::MAINNET_DEFAULT_INDEXER);
        let testnet = host_of(default_indexer::TESTNET_DEFAULT_INDEXER);

        let first = new_runtime()?;
        let before = first.block_on(lookup(&mainnet))?;
        drop(first);
        let after = new_runtime()?.block_on(lookup(&testnet))?;

        assert!(
            !before.is_empty() && !after.is_empty(),
            "{mainnet} answered {before:?} and {testnet} answered {after:?}"
        );
        Ok(())
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
    async fn local_names_answer_without_a_query() {
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
    }

    #[tokio::test]
    #[ignore = "resolves a live host name through Cloudflare and Quad9"]
    async fn the_mainnet_default_indexer_resolves_over_doh_dot() -> io::Result<()> {
        let host = host_of(default_indexer::MAINNET_DEFAULT_INDEXER);
        let public = lookup(&host).await?;
        assert!(
            !public.is_empty(),
            "{host} must resolve over DoH/DoT, got {public:?}"
        );
        Ok(())
    }
}

//! Embedded Nym SOCKS5 proxy for routing gRPC traffic through the Nym mixnet.
//!
//! This module wraps the nym-sdk `Socks5MixnetClient` lifecycle and provides
//! auto-discovery of public exit gateways (network requesters). It is gated
//! on the `nym` feature, whose dependencies resolve only in this crate's own
//! lockfile — never the parent workspace's — because nym-sdk's transitive
//! graph requires `crypto-common ^0.2`, which cannot coexist with the parent
//! workspace's `crypto-common =0.2.0-rc.1` pin. See
//! `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! # Architecture
//!
//! The Nym mixnet fragments traffic into Sphinx packets, shuffles them
//! through a three-layer mix network, and reassembles at an exit gateway.
//! The exit gateway runs a "network requester" service that makes the actual
//! TCP connections to the target server on behalf of the client.
//!
//! [`NymProxy`] embeds an in-process SOCKS5 proxy that connects to the
//! mixnet and listens on a localhost port. A consumer routes gRPC (or any
//! TCP) traffic through that local SOCKS5 address; the wallet-side transport
//! that dials it lives in the main workspace and needs only a SOCKS5 client,
//! not this nym-sdk stack.
//!
//! # Lifecycle
//!
//! 1. **Start**: [`NymProxy::start`] discovers public exit gateways and
//!    connects to the mixnet, retrying across multiple gateways.
//! 2. **Validate**: [`NymProxy::check_connectivity`] opens a test TCP tunnel
//!    through the proxy to verify end-to-end reachability of a target.
//! 3. **Use**: read the local SOCKS5 address from [`NymProxy::socks5_addr`].
//! 4. **Reconnect**: [`NymProxy::reconnect`] starts a fresh client on a new
//!    port, then disconnects the old one.
//! 5. **Disconnect**: [`NymProxy::disconnect`] shuts down the client cleanly.
#![forbid(unsafe_code)]

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use nym_sdk::mixnet::{MixnetClientBuilder, Socks5, Socks5MixnetClient};
use tokio::time::sleep;

use crate::error::NymProxyError;
use crate::mixnet_connect::{connect_with_retries, seeded_shuffle, strip_socks5_scheme};

/// Default Nym API URL for mainnet.
const DEFAULT_NYM_API_URL: &str = "https://validator.nymtech.net/api/";

/// Maximum number of providers to try before giving up.
const MAX_PROVIDER_ATTEMPTS: usize = 10;

/// Maximum number of connection attempts per provider set.
const MAX_CONNECTION_ATTEMPTS: usize = 10;

/// Sleep between retry rounds (milliseconds).
const SYSTEM_SLEEP_MILLIS: u64 = 100;

/// Overall timeout for `start()` and `reconnect()` to prevent infinite hangs.
///
/// Nym SDK connection attempts can block indefinitely if a gateway is
/// unresponsive. This timeout caps total wall-clock time for the entire
/// retry loop, not individual attempts.
const NYM_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Embedded Nym SOCKS5 proxy that routes traffic through the Nym mixnet.
///
/// Manages the lifecycle of an in-process Nym SOCKS5 client connected to a
/// public exit gateway. The proxy listens on a localhost port.
pub struct NymProxy {
    client: Socks5MixnetClient,
    bind_port: u16,
}

impl NymProxy {
    /// Start an embedded Nym SOCKS5 proxy using an auto-discovered public exit gateway.
    ///
    /// Queries the Nym API for active exit gateways, then tries up to 10
    /// gateways across 10 connection rounds before giving up. The proxy
    /// listens on a random available localhost port. This is the recommended
    /// entry point — no Nym-specific addresses are required.
    pub async fn start() -> Result<Self, NymProxyError> {
        tokio::time::timeout(NYM_LIFECYCLE_TIMEOUT, Self::start_inner())
            .await
            .map_err(|_| {
                NymProxyError::ConnectivityCheck(format!(
                    "start timed out after {}s",
                    NYM_LIFECYCLE_TIMEOUT.as_secs()
                ))
            })?
    }

    async fn start_inner() -> Result<Self, NymProxyError> {
        let providers = Self::discover_providers(DEFAULT_NYM_API_URL).await?;
        let port = Self::find_available_port()?;
        Self::connect_across_providers(&providers, port).await
    }

    /// Run the shared retry engine over `providers`, connecting each attempt
    /// on `port`. Shared by [`Self::start`] and [`Self::reconnect`].
    async fn connect_across_providers(
        providers: &[String],
        port: u16,
    ) -> Result<Self, NymProxyError> {
        connect_with_retries(
            providers,
            MAX_PROVIDER_ATTEMPTS,
            MAX_CONNECTION_ATTEMPTS,
            Duration::from_millis(SYSTEM_SLEEP_MILLIS),
            move |provider: String| async move { Self::start_with_config(&provider, port).await },
            sleep,
        )
        .await
        .map_err(|last_err| last_err.unwrap_or(NymProxyError::NoProvider))
    }

    /// Start with a specific exit gateway provider address.
    ///
    /// Use this to pin a specific Nym network requester instead of
    /// auto-discovering one. The `provider_mix_address` is a Nym `Recipient`
    /// address in base58 format (`<client_id>.<client_enc>@<gateway_id>`).
    /// Listens on a random available localhost port.
    pub async fn start_with_provider(provider_mix_address: &str) -> Result<Self, NymProxyError> {
        let port = Self::find_available_port()?;
        Self::start_with_config(provider_mix_address, port).await
    }

    /// Start with a specific provider and custom local bind port.
    ///
    /// Useful when running multiple Nym proxies or when a specific port is
    /// required.
    pub async fn start_with_config(
        provider_mix_address: &str,
        bind_port: u16,
    ) -> Result<Self, NymProxyError> {
        let socks5_cfg = Socks5::new(provider_mix_address);
        let client = MixnetClientBuilder::new_ephemeral()
            .socks5_config(Socks5 {
                bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bind_port),
                ..socks5_cfg
            })
            .build()
            .map_err(|e| NymProxyError::Build(Box::new(e)))?
            .connect_to_mixnet_via_socks5()
            .await
            .map_err(|e| NymProxyError::Connect(Box::new(e)))?;
        Ok(Self { client, bind_port })
    }

    /// The local SOCKS5 proxy address (e.g., `"127.0.0.1:43210"`).
    pub fn socks5_addr(&self) -> String {
        strip_socks5_scheme(&self.client.socks5_url()).to_string()
    }

    /// The local bind port the SOCKS5 proxy listens on.
    pub fn bind_port(&self) -> u16 {
        self.bind_port
    }

    /// Verify that a TCP connection can be established through this proxy to
    /// the given target host and port.
    ///
    /// Opens a SOCKS5 tunnel to the target, verifying end-to-end reachability
    /// through the Nym mixnet. The connection is dropped immediately after
    /// success.
    pub async fn check_connectivity(
        &self,
        target_host: &str,
        target_port: u16,
    ) -> Result<(), NymProxyError> {
        let addr = self.socks5_addr();
        let _stream = tokio_socks::tcp::Socks5Stream::connect(&*addr, (target_host, target_port))
            .await
            .map_err(|e| NymProxyError::ConnectivityCheck(e.to_string()))?;
        Ok(())
    }

    /// Disconnect the current mixnet client and start a fresh one.
    ///
    /// Rediscovers providers and connects on a **new** local port to avoid
    /// binding conflicts with the still-running old client. The old client is
    /// disconnected only after the new one succeeds. If all connection
    /// attempts fail, the old client remains untouched and the error is
    /// returned. After a successful reconnect, [`socks5_addr`](Self::socks5_addr)
    /// returns the new port.
    pub async fn reconnect(&mut self) -> Result<(), NymProxyError> {
        tokio::time::timeout(NYM_LIFECYCLE_TIMEOUT, self.reconnect_inner())
            .await
            .map_err(|_| {
                NymProxyError::ConnectivityCheck(format!(
                    "reconnect timed out after {}s",
                    NYM_LIFECYCLE_TIMEOUT.as_secs()
                ))
            })?
    }

    async fn reconnect_inner(&mut self) -> Result<(), NymProxyError> {
        // Use a new port so we don't conflict with the old client's bind.
        let new_port = Self::find_available_port()?;
        let providers = Self::discover_providers(DEFAULT_NYM_API_URL).await?;
        let new_proxy = Self::connect_across_providers(&providers, new_port).await?;

        // Swap only after the new client succeeded, so a failed reconnect
        // leaves the old client untouched.
        let old_client = std::mem::replace(&mut self.client, new_proxy.client);
        self.bind_port = new_port;
        old_client.disconnect().await;
        Ok(())
    }

    /// Disconnect from the Nym mixnet and stop the local SOCKS5 proxy.
    pub async fn disconnect(self) {
        self.client.disconnect().await;
    }

    /// Find an available localhost port by briefly binding to port 0.
    ///
    /// # TOCTOU race
    ///
    /// There is an inherent race between dropping the listener and the Nym
    /// SDK rebinding to the same port: another process could claim it in
    /// between. This is a fundamental limitation of the bind-to-0-then-drop
    /// pattern (also used by `portpicker`). In practice the race is extremely
    /// unlikely and causes a connection retry, not a security issue, since
    /// `start()` retries across multiple gateways.
    fn find_available_port() -> Result<u16, NymProxyError> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| {
            NymProxyError::DiscoveryApi(format!("failed to find available port: {e}"))
        })?;
        let port = listener
            .local_addr()
            .map_err(|e| NymProxyError::DiscoveryApi(format!("failed to get port: {e}")))?
            .port();
        drop(listener);
        Ok(port)
    }

    /// Query the Nym API for active exit gateways running a network requester.
    ///
    /// Returns addresses shuffled for load distribution
    /// ([`seeded_shuffle`] on [`time_entropy_seed`] — see its docs for why
    /// this is deliberately not cryptographic randomness). Callers should try
    /// multiple entries since individual gateways may be offline.
    async fn discover_providers(nym_api_url: &str) -> Result<Vec<String>, NymProxyError> {
        use nym_validator_client::nym_api::NymApiClientExt as _;

        let api_client = nym_http_api_client::Client::builder(nym_api_url)
            .map_err(|e| NymProxyError::DiscoveryApi(e.to_string()))?
            .build()
            .map_err(|e| NymProxyError::DiscoveryApi(e.to_string()))?;

        let described_nodes = api_client
            .get_all_described_nodes_v2()
            .await
            .map_err(|e| NymProxyError::DiscoveryApi(e.to_string()))?;

        // Collect all nodes that have a network requester with an address.
        let mut providers: Vec<String> = described_nodes
            .iter()
            .filter_map(|node| node.description.network_requester.as_ref())
            .map(|nr| nr.address.clone())
            .filter(|addr| !addr.is_empty())
            .collect();

        if providers.is_empty() {
            return Err(NymProxyError::NoProvider);
        }

        seeded_shuffle(&mut providers, time_entropy_seed());
        Ok(providers)
    }
}

/// The entropy source for provider shuffling: a hash of the current time.
/// The one effect feeding the pure [`seeded_shuffle`].
fn time_entropy_seed() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The scheme-stripping and retry-engine logic is tested in
    // `mixnet_connect`, where the tests call the REAL functions in the
    // default build; the earlier copies of that logic here tested a
    // transcription of the expression, not the code.

    #[test]
    fn find_available_port_returns_nonzero() {
        let port = NymProxy::find_available_port().expect("find_available_port");
        assert!(port > 0);
    }

    // Integration tests below require a live Nym network. Run with:
    //   cargo test --manifest-path zingo-netutils/Cargo.toml --features nym -- --ignored

    /// Start the embedded proxy and verify it reports a valid localhost address.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires live Nym network"]
    async fn nym_proxy_starts_and_reports_address() {
        let proxy = NymProxy::start().await.expect("NymProxy::start");
        let addr = proxy.socks5_addr();
        assert!(
            addr.starts_with("127.0.0.1:"),
            "expected localhost address, got {addr}"
        );
        let port: u16 = addr
            .split(':')
            .next_back()
            .unwrap()
            .parse()
            .expect("port should be numeric");
        assert!(port > 0, "port should be non-zero");
        proxy.disconnect().await;
    }

    /// Start the proxy and verify a SOCKS5 TCP tunnel works end-to-end.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires live Nym network"]
    async fn nym_proxy_socks5_tunnel_works() {
        let proxy = NymProxy::start().await.expect("NymProxy::start");
        let addr = proxy.socks5_addr();

        let stream = tokio_socks::tcp::Socks5Stream::connect(&*addr, "zec.rocks:443")
            .await
            .expect("SOCKS5 connect");

        drop(stream);
        proxy.disconnect().await;
    }

    /// Start and disconnect cleanly with no panic.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires live Nym network"]
    async fn nym_proxy_disconnect_clean() {
        let proxy = NymProxy::start().await.expect("NymProxy::start");
        proxy.disconnect().await;
    }
}

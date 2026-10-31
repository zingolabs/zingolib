//! Pinned voting-config resolution over the SDK's authenticated channels.
//!
//! The static trust anchor is a hash-pinned document naming the trusted keys;
//! the dynamic config mirrors carry the rounds and endpoints, each round
//! attested by those keys. Resolution goes through `zcash_voting`'s resolver,
//! so trust is inherited from the SDK and never re-derived here. The staging
//! source sits behind one clearly-named constant: production pinning is a
//! one-line swap of [`STAGING_STATIC_VOTING_CONFIG_SOURCE`].

use std::time::Duration;

use zcash_voting::config::{
    DYNAMIC_MIRROR_FETCH_TIMEOUT, PinnedConfigSource, ResolveVotingConfigOptions,
    ResolvedVotingConfig, resolve_dynamic_voting_config_over_mirrors, resolve_static_voting_config,
};
use zcash_voting::{DirectRoute, RouteHttp, RouteRequest, RouteResponse};

use crate::error::VotingFailure;

/// The staging trust anchor. A production release pins its own source string
/// here; nothing else in the crate names a config source.
pub const STAGING_STATIC_VOTING_CONFIG_SOURCE: &str =
    "https://voting.valargroup.dev/stage/static-voting-config.json";

/// The production trust anchor: the default source mainnet wallets resolve.
/// Pinned to the immutable copy the publisher requires for wallet releases
/// (token-holder-voting-config: `pins/prod/<sha256>/`, checksum-verified).
/// A publisher static-config change (key rotation) ships with a new pin.
pub const PRODUCTION_STATIC_VOTING_CONFIG_SOURCE: &str = "https://voting.valargroup.dev/pins/prod/fb62a56fae28debfdaa092f163cda0dab13295f87d25bbc4d0064d6ccdeb6943/static-voting-config.json?checksum=sha256:fb62a56fae28debfdaa092f163cda0dab13295f87d25bbc4d0064d6ccdeb6943";

/// Overrides the config source for one process: deployments and demos point
/// a wallet at another network (staging, a local stack) without a rebuild.
/// Set before the wallet opens; beats the caller's configured source.
pub const STATIC_CONFIG_SOURCE_ENV: &str = "ZINGO_VOTING_STATIC_CONFIG_SOURCE";

/// Response-body ceiling for config fetches.
const MAX_CONFIG_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Request deadline for one config fetch.
const CONFIG_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Fetches `url` over the SDK's direct HTTPS route as a bounded GET.
async fn fetch_bytes(route: &DirectRoute, url: String) -> Result<Vec<u8>, String> {
    let request = RouteRequest {
        method: http::Method::GET,
        url: &url,
        headers: &[("Cache-Control".to_string(), "no-cache".to_string())],
        body: Vec::new(),
        timeout: CONFIG_FETCH_TIMEOUT,
        connect_timeout: None,
        max_response_bytes: MAX_CONFIG_RESPONSE_BYTES,
    };
    let on_dispatch = || {};
    let response: RouteResponse = RouteHttp::execute(route, request, &on_dispatch)
        .await
        .map_err(|error| error.message)?;
    if !(200..300).contains(&response.status) {
        return Err(format!("config fetch returned HTTP {}", response.status));
    }
    Ok(response.body)
}

/// Resolves the pinned static source through its dynamic mirrors, with every
/// round authenticated against the static keys.
///
/// # Errors
///
/// Returns [`VotingFailure::Config`] when the source does not parse, the hash
/// pin does not match, or every mirror fails; [`VotingFailure::VoteServer`]
/// when the transport itself cannot route.
pub async fn resolve_voting_config(
    route: &DirectRoute,
    source: &str,
) -> Result<ResolvedVotingConfig, VotingFailure> {
    let source = std::env::var(STATIC_CONFIG_SOURCE_ENV).unwrap_or_else(|_| source.to_string());
    let static_url = PinnedConfigSource::parse(&source)
        .map_err(|error| VotingFailure::Config(error.to_string()))?
        .url;
    let static_bytes = fetch_bytes(route, static_url).await.map_err(|error| {
        VotingFailure::VoteServer(format!("voting config source {source}: {error}"))
    })?;
    let resolved_static = resolve_static_voting_config(&source, &static_bytes)
        .map_err(|error| VotingFailure::Config(error.to_string()))?;

    let (resolved, _skipped_mirrors) = resolve_dynamic_voting_config_over_mirrors(
        resolved_static,
        DYNAMIC_MIRROR_FETCH_TIMEOUT,
        ResolveVotingConfigOptions::default(),
        |url| fetch_bytes(route, url),
    )
    .await
    .map_err(|error| VotingFailure::Config(error.to_string()))?;

    Ok(resolved)
}

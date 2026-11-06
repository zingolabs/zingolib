//! One HTTPS GET over the mixnet on the fetch exit, for a consumer that
//! needs third-party data (a forex table, a fee estimate) without a
//! nakednet leg and without sharing the Standing Client's exit.
//!
//! The price fetch is one use of the same run: `FetchRun` contains the
//! exit acquisition, the disposal, and the narration that every fetch on
//! the fetch exit shares, and `FetchTargets` supplies what one fetch
//! races and how it reads the responses.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use zingo_netutils::conduit::ConduitDial;
use zingo_netutils::socks5_fetch::Socks5FetchError;

use crate::mixnet::acquire::TransportError;
use crate::mixnet::pools::Pools;
use crate::mixnet::pools::exit_pool::ExitNodeHealthVerdict;
use crate::mixnet::roles::Role;
use crate::mixnet::speed::{Member, SpeedError, SpeedPrioritized, SpeedProgress};

/// The client-side bound on establishing one fetch connection alone,
/// so a request timeout above it still refuses an exit that never
/// completes the handshake.
pub const FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What one fetch on the fetch exit races, and how it reads the responses.
pub(crate) trait FetchTargets: Send + Sync {
    /// What this fetch races: a price source, a consumer's URL.
    type Target: Send;

    /// What one target returns, whether it answered or failed.
    type Response: Send;

    /// The label the run logs under.
    fn log_label(&self) -> &'static str;

    /// The targets this fetch races, in the order the wave takes them.
    fn targets(&self) -> Vec<Self::Target>;

    /// Fetches one target through `dial`'s conduit.
    fn probe(
        &self,
        dial: ConduitDial,
        target: Self::Target,
    ) -> impl std::future::Future<Output = Self::Response> + Send;

    /// Whether any target answered, which both settles the wave and
    /// proves the exit alive.
    fn answered(&self, responses: &[Self::Response]) -> bool;
}

/// A fetch on the fetch exit: one speed-priority wave over a client
/// created for this run, so consecutive fetches never share a client.
pub(crate) struct FetchRun<F> {
    pools: Arc<Pools>,
    targets: F,
}

impl<F: FetchTargets> FetchRun<F> {
    /// A run taking its transport from `pools` and racing `targets`.
    pub(crate) fn new(pools: Arc<Pools>, targets: F) -> Self {
        FetchRun { pools, targets }
    }

    fn retire(&self, member: Member, health: ExitNodeHealthVerdict) {
        self.pools.remember(member.node().clone(), health);
        tokio::spawn(async move {
            member.retire().await;
        });
    }
}

impl<F: FetchTargets> SpeedPrioritized for FetchRun<F> {
    type Target = F::Target;
    type Outcome = F::Response;

    fn targets(&self) -> Vec<Self::Target> {
        self.targets.targets()
    }

    async fn probe(&self, dial: ConduitDial, target: Self::Target) -> Self::Outcome {
        self.targets.probe(dial, target).await
    }

    fn settled(&self, responses: &[Self::Outcome]) -> bool {
        SpeedPrioritized::answered(self, responses)
    }

    fn answered(&self, responses: &[Self::Outcome]) -> bool {
        self.targets.answered(responses)
    }

    fn acquire(
        &self,
    ) -> impl std::future::Future<Output = Result<(Member, std::net::SocketAddr), TransportError>> + Send
    {
        let pools = self.pools.clone();
        async move {
            let acquirer = pools.acquirer().ok_or(TransportError::NoAcquirer)?;
            let created = pools
                .conduit_for(Role::PriceFetch, acquirer.as_ref())
                .await?;
            let member = Member::new(created.transport, created.lease);
            let addr = member.addr().ok_or(TransportError::DiedBeforeUse)?;
            Ok((member, addr))
        }
    }

    fn dispose(&self, spent: Member) {
        self.retire(spent, ExitNodeHealthVerdict::EpochProven);
    }

    fn abandon(&self, dead: Member) {
        tracing::warn!(
            "{}: exit {:?} abandoned, no target answered",
            self.targets.log_label(),
            dead.node()
        );
        self.retire(dead, ExitNodeHealthVerdict::Failed);
    }

    fn narrate(&self, phase: SpeedProgress) {
        tracing::info!("{}: {phase:?}", self.targets.log_label());
    }
}

/// A consumer's one URL, fetched as text under the caller's request
/// timeout.
pub(crate) struct UrlFetch {
    /// The URL to GET. Admitted by [`crate::mixnet::probe::probe_eligible`]
    /// before the run.
    pub(crate) url: http::Uri,
    /// The client-side bound on the whole request.
    pub(crate) timeout: Duration,
}

impl FetchTargets for UrlFetch {
    type Target = http::Uri;
    type Response = Result<String, Socks5FetchError>;

    fn log_label(&self) -> &'static str {
        "mixnet fetch"
    }

    fn targets(&self) -> Vec<Self::Target> {
        vec![self.url.clone()]
    }

    async fn probe(&self, dial: ConduitDial, target: Self::Target) -> Self::Response {
        dial.fetch_text(
            &target.to_string(),
            self.timeout,
            self.timeout.min(FETCH_CONNECT_TIMEOUT),
        )
        .await
    }

    fn answered(&self, responses: &[Self::Response]) -> bool {
        responses.iter().any(Result::is_ok)
    }
}

/// The body one fetch returned, with the route it traveled and the time
/// it took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixnetFetch {
    /// The response body as text.
    pub body: String,
    /// Wall-clock time from dispatch to the answer, tunnel traversal
    /// included.
    pub round_trip: Duration,
    /// The local SOCKS5 endpoint the fetch traveled through.
    pub via_socks5: String,
}

/// Why a fetch over the mixnet returned no body.
#[derive(Debug, thiserror::Error)]
pub enum MixnetFetchError {
    /// A URL outside the one endpoint shape the exit policy carries.
    #[error("fetch targets must be https on port 443; got '{0}'")]
    Ineligible(http::Uri),
    /// The run acquired no transport, or every exit it drew carried
    /// nothing.
    #[error("the fetch reached no exit.")]
    Speed(#[source] SpeedError),
    /// The request traveled and failed at a typed stage.
    #[error("the fetch failed.")]
    Request(#[source] Box<Socks5FetchError>),
    /// The wave ended with no response for the one URL.
    #[error("the fetch produced no response.")]
    NoResponse,
}

/// The body among a single-URL wave's responses, or the typed failure.
pub(crate) fn first_body(
    responses: Vec<Result<String, Socks5FetchError>>,
) -> Result<String, MixnetFetchError> {
    responses
        .into_iter()
        .next()
        .ok_or(MixnetFetchError::NoResponse)?
        .map_err(|error| MixnetFetchError::Request(Box::new(error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(text: &str) -> http::Uri {
        text.parse().expect("a well-formed test uri")
    }

    #[test]
    fn a_url_fetch_races_its_one_url() {
        let fetch = UrlFetch {
            url: uri("https://example.test/rates"),
            timeout: Duration::from_secs(5),
        };
        assert_eq!(fetch.targets(), vec![uri("https://example.test/rates")]);
        assert!(!fetch.answered(&[]));
        assert!(fetch.answered(&[Ok("{}".to_string())]));
    }

    #[test]
    fn an_empty_wave_is_a_typed_absence() {
        assert!(matches!(
            first_body(Vec::new()),
            Err(MixnetFetchError::NoResponse)
        ));
        assert_eq!(first_body(vec![Ok("{}".to_string())]).unwrap(), "{}");
    }
}

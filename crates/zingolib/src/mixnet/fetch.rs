//! An HTTPS GET request over the mixnet for a consumer.
//!
//! The call makes one attempt and records exit health only when a
//! response arrives.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use zl_netutils::conduit::{ConduitDial, MixnetConduit};
use zl_netutils::socks5_fetch::{HttpResponse, HttpResponseError, Socks5FetchError};

use crate::mixnet::acquire::TransportError;
use crate::mixnet::pools::Pools;
use crate::mixnet::pools::exit_pool::ExitNodeHealthVerdict;
use crate::mixnet::roles::Role;
use crate::mixnet::speed::Member;

/// The maximum time to establish the connection.
pub const FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The maximum size of a response body, in bytes.
pub const FETCH_BODY_LIMIT: usize = 1024 * 1024;

/// Acquires a client on the fetch exit and its SOCKS5 address. The first
/// run of a session uses the client that boot verified. Each later run
/// creates a new client.
pub(crate) async fn acquire_fetch_client(
    pools: Arc<Pools>,
) -> Result<(Member, SocketAddr), TransportError> {
    let acquirer = pools.acquirer().ok_or(TransportError::NoAcquirer)?;
    let created = pools
        .conduit_for(Role::PriceFetch, acquirer.as_ref())
        .await?;
    let member = Member::new(created.transport, created.lease);
    let addr = member.addr().ok_or(TransportError::DiedBeforeUse)?;
    Ok((member, addr))
}

/// Closes `member` without recording exit health.
fn close_unrecorded(member: Member) {
    tokio::spawn(async move {
        member.retire().await;
    });
}

/// Records `health` for the exit of `member`, then closes `member`.
pub(crate) fn close_with_health(pools: &Pools, member: Member, health: ExitNodeHealthVerdict) {
    pools.remember(member.node().clone(), health);
    close_unrecorded(member);
}

/// Returns the exit health that a request result shows. A failure shows
/// nothing.
fn exit_health_after<T, E>(result: &Result<T, E>) -> Option<ExitNodeHealthVerdict> {
    result.is_ok().then_some(ExitNodeHealthVerdict::EpochProven)
}

/// The result of a fetch over the mixnet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixnetFetch {
    /// The HTTP status code.
    pub status: u16,
    /// The response body as text.
    pub body: String,
    /// The time from the start of the call to the response.
    pub round_trip: Duration,
    /// The local SOCKS5 address that the request used.
    pub via_socks5: String,
}

/// The reason a fetch over the mixnet returned no response.
#[derive(Debug, thiserror::Error)]
pub enum MixnetFetchError {
    /// The URL is not `https` on port 443.
    #[error("fetch targets must be https on port 443; got '{0}'")]
    Ineligible(http::Uri),
    /// The timeout is zero.
    #[error("the fetch timeout must be greater than zero")]
    ZeroTimeout,
    /// The deadline passed before the request started.
    #[error("the fetch exceeded its {}ms deadline before the request started", timeout.as_millis())]
    DeadlineExceeded {
        /// The timeout of the call.
        timeout: Duration,
    },
    /// No client could be acquired.
    #[error("the fetch acquired no client.")]
    Transport(#[source] TransportError),
    /// The request failed.
    #[error("the fetch failed.")]
    Request(#[source] Box<Socks5FetchError>),
    /// The body is larger than [`FETCH_BODY_LIMIT`].
    #[error("the response body is larger than {limit} bytes")]
    BodyTooLarge {
        /// The limit in bytes.
        limit: usize,
    },
}

impl From<HttpResponseError> for MixnetFetchError {
    fn from(error: HttpResponseError) -> Self {
        match error {
            HttpResponseError::Transport(failure) => Self::Request(Box::new(failure)),
            HttpResponseError::BodyTooLarge { limit } => Self::BodyTooLarge { limit },
        }
    }
}

async fn request_by_deadline(
    dial: ConduitDial,
    url: &http::Uri,
    deadline: tokio::time::Instant,
    timeout: Duration,
) -> Result<HttpResponse, MixnetFetchError> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Err(MixnetFetchError::DeadlineExceeded { timeout });
    }
    Ok(dial
        .fetch_response(
            &url.to_string(),
            remaining,
            remaining.min(FETCH_CONNECT_TIMEOUT),
            FETCH_BODY_LIMIT,
        )
        .await?)
}

/// Sends one GET request to `url` within `timeout`. It uses a client
/// acquired for this call, or `session_dial` if the session cannot create
/// clients.
pub(crate) async fn fetch_url(
    pools: &Arc<Pools>,
    session_dial: ConduitDial,
    url: &http::Uri,
    timeout: Duration,
) -> Result<MixnetFetch, MixnetFetchError> {
    let dispatched = std::time::Instant::now();
    let deadline = tokio::time::Instant::now() + timeout;
    let acquired = if pools.acquirer().is_some() {
        let client = tokio::time::timeout_at(deadline, acquire_fetch_client(pools.clone()))
            .await
            .map_err(|_| MixnetFetchError::DeadlineExceeded { timeout })?
            .map_err(MixnetFetchError::Transport)?;
        Some(client)
    } else {
        None
    };
    let dial = match &acquired {
        Some((_, addr)) => MixnetConduit::over(*addr).dial(),
        None => session_dial,
    };
    let via_socks5 = dial.socks5().to_string();
    let response = request_by_deadline(dial, url, deadline, timeout).await;
    if let Some((member, _)) = acquired {
        match exit_health_after(&response) {
            Some(health) => close_with_health(pools, member, health),
            None => close_unrecorded(member),
        }
    }
    let response = response?;
    Ok(MixnetFetch {
        status: response.status,
        body: response.body,
        round_trip: dispatched.elapsed(),
        via_socks5,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_marks_the_exit_healthy_and_a_failure_records_nothing() {
        let responded: Result<u16, ()> = Ok(404);
        assert_eq!(
            exit_health_after(&responded),
            Some(ExitNodeHealthVerdict::EpochProven),
            "an error status still traveled through the exit"
        );

        let failed: Result<u16, ()> = Err(());
        assert_eq!(
            exit_health_after(&failed),
            None,
            "one URL that does not respond is no evidence about the exit"
        );
    }

    #[test]
    fn a_body_over_the_limit_keeps_its_limit_across_the_conversion() {
        let converted = MixnetFetchError::from(HttpResponseError::BodyTooLarge { limit: 8 });
        assert!(matches!(
            converted,
            MixnetFetchError::BodyTooLarge { limit: 8 }
        ));
    }

    #[tokio::test]
    async fn a_deadline_that_already_elapsed_refuses_before_any_request() {
        let conduit = MixnetConduit::over("127.0.0.1:1".parse().expect("a static address"));
        let url: http::Uri = "https://rates.test/latest".parse().expect("static uri");
        let timeout = Duration::from_millis(1);

        let error = request_by_deadline(conduit.dial(), &url, tokio::time::Instant::now(), timeout)
            .await
            .expect_err("no time remains for the request");
        assert!(
            matches!(error, MixnetFetchError::DeadlineExceeded { timeout: given } if given == timeout),
            "the refusal must report the caller's deadline: {error}"
        );
    }
}

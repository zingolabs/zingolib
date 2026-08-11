//! The Server-Selection Sweep runner (ADR 0034): the impure half that emits
//! the survey and turns it into a sync indexer.
//!
//! The runner spawns a dedicated sweep proxy — its own `nym-proxy` child on
//! its own status channel, so its bootstrap never churns the session's
//! send/price-fetch transport and its Exit Node is distinct by construction.
//! It surveys the candidates through that exit, hands the results to the
//! pure [`sweep::select`], and drops the proxy, which recycles its exit: the
//! transport that learned what was surveyed carries nothing after.
#![forbid(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use http::Uri;

use super::LightClient;
use crate::mixnet::MixnetMode;
use crate::mixnet::probe::ProbeSuccess;
use crate::mixnet::sweep::{self, Selection, SurveyResult, SweepError};

/// Two blocks: the height tolerance around the observed median that counts
/// as live (ADR 0034).
pub const SWEEP_HEIGHT_TOLERANCE: u64 = 2;

/// A phase transition of a running Server-Selection Sweep, delivered to the
/// consumer's progress callback as the sweep reaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SweepProgress {
    /// The dedicated sweep transport is bootstrapping toward its own exit.
    TransportBootstrapping,
    /// The survey is probing every candidate through the sweep exit.
    Surveying {
        /// How many candidates the survey covers.
        candidates: usize,
    },
    /// The survey finished and the pure judgment is running.
    Judging {
        /// How many candidates answered the survey.
        answered: usize,
        /// How many candidates were surveyed.
        surveyed: usize,
    },
}

/// Why a Server-Selection Sweep produced no sync indexer.
#[derive(Debug, thiserror::Error)]
pub enum ServerSelectionError {
    /// The sweep could not draw a ledgered Clutch for its transport.
    #[error("the sweep could not acquire a transport: {0}")]
    TransportAcquisition(#[source] crate::mixnet::acquire::TransportError),
    /// The dedicated sweep proxy could not be spawned.
    #[error("the sweep proxy could not start: {0}")]
    ProxyStart(#[source] crate::mixnet::MixnetProxyError),
    /// The sweep proxy did not reach readiness: it died or exceeded its
    /// bootstrap budget before any survey ran.
    #[error("the sweep transport did not become ready: {0}")]
    TransportUnready(String),
    /// The survey ran but no sync indexer could be selected.
    #[error(transparent)]
    Selection(#[from] SweepError),
}

impl LightClient {
    /// Run one Server-Selection Sweep (ADR 0034) over `candidates`, returning
    /// the selected sync indexer, the transmit candidates that exclude its
    /// operator, and the height-ordered live cohort.
    ///
    /// `binary_path` is the `nym-proxy` binary the dedicated sweep proxy
    /// spawns from. `pin` is an explicit user server: it is surveyed like any
    /// candidate and selected when live, and its absence from the live cohort
    /// fails [`SweepError::DeadPin`] rather than falling back to the draw.
    ///
    /// The sweep proxy is dropped before this returns, recycling its exit.
    pub async fn run_server_selection_sweep(
        &self,
        binary_path: &Path,
        candidates: &[Uri],
        pin: Option<&Uri>,
        progress: impl Fn(SweepProgress),
    ) -> Result<Selection, ServerSelectionError> {
        let chain = lightd_chain_name(&self.chain_type());
        // A dedicated status channel: the sweep proxy's lifecycle is private
        // to this call and must not touch the session's mixnet status.
        let publisher = crate::mixnet::status_publisher();
        let mut receiver = publisher.subscribe();
        // The sweep gates the Sync Session a user just asked to open.
        use zingo_netutils::responsiveness::{PrioritiseSpeed, Responsiveness as _};
        let acquirer = crate::mixnet::acquire::SpawnedBinary::at(binary_path.to_path_buf());
        // The sweep refuses without a ledgered Clutch; its reservations are
        // held for the sweep's life and recycled by drop on every return.
        let mut clutch = self
            .correspondent_pools
            .draw_clutch(&acquirer)
            .await
            .map_err(ServerSelectionError::TransportAcquisition)?;
        let nodes = crate::correspondent::pool::exit_pool::clutch_nodes(&clutch);
        let proxy = crate::mixnet::acquire::TransportAcquirable::acquire(
            &acquirer,
            PrioritiseSpeed::CLASS,
            &nodes,
            publisher,
        )
        .await
        .map_err(ServerSelectionError::TransportAcquisition)?;

        progress(SweepProgress::TransportBootstrapping);
        let (socks5_addr, exits) = await_sweep_ready(&mut receiver).await?;
        // Bind-time recycle: the survey's fan-out is a declared Shared use
        // of the one bound exit, and the unbound reservations return now.
        let bound = clutch
            .iter()
            .position(|reservation| exits.contains(reservation.node()))
            .expect("the bound exit is one of the clutch's nodes");
        let lease = clutch.swap_remove(bound);
        drop(clutch);
        let member: crate::correspondent::pool::Member<
            crate::mixnet::MixnetProxy,
            crate::correspondent::pool::Shared,
        > = crate::correspondent::pool::Member::new(proxy, lease);
        progress(SweepProgress::Surveying {
            candidates: candidates.len(),
        });
        let results = survey(socks5_addr, candidates, &self.indexer_history).await;
        progress(SweepProgress::Judging {
            answered: results.iter().filter(|r| r.reported.is_some()).count(),
            surveyed: results.len(),
        });

        let selection = sweep::select(
            &results,
            chain,
            SWEEP_HEIGHT_TOLERANCE,
            pin,
            &mut rand::rngs::OsRng,
        )?;

        // Exit Recycling: retiring the member kills the child and recycles
        // its lease, so no later traffic rides the exit that observed the
        // survey.
        member.retire().await;
        Ok(selection)
    }
}

/// The chain name a `GetLightdInfo` reply carries for `chain`, the
/// vocabulary the survey's liveness judgment must compare against.
fn lightd_chain_name(chain: &crate::config::ChainType) -> &'static str {
    match chain {
        crate::config::ChainType::Mainnet => "main",
        crate::config::ChainType::Testnet => "test",
        crate::config::ChainType::Regtest(_) => "regtest",
    }
}

/// Wait for the dedicated sweep proxy to reach `Ready` and yield its SOCKS5
/// address with its bound Exit Nodes, or fail typed when it dies or its
/// bootstrap budget elapses.
async fn await_sweep_ready(
    receiver: &mut tokio::sync::watch::Receiver<crate::mixnet::MixnetStatus>,
) -> Result<(std::net::SocketAddr, Vec<crate::mixnet::ExitNodeId>), ServerSelectionError> {
    let budget = zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT;
    let outcome = tokio::time::timeout(budget, async {
        loop {
            {
                let status = receiver.borrow_and_update();
                match status.mode {
                    MixnetMode::Ready => {
                        if let Some(addr) = status.socks5_addr {
                            return Ok((addr, status.exits.clone()));
                        }
                    }
                    MixnetMode::Died => {
                        return Err(status
                            .death
                            .as_ref()
                            .and_then(|d| d.detail.as_ref().map(std::string::ToString::to_string))
                            .unwrap_or_else(|| {
                                "the sweep proxy died during bootstrap".to_string()
                            }));
                    }
                    MixnetMode::Unattached
                    | MixnetMode::SwitchedOff
                    | MixnetMode::Bootstrapping => {}
                }
            }
            if receiver.changed().await.is_err() {
                return Err("the sweep proxy status channel closed".to_string());
            }
        }
    })
    .await;
    match outcome {
        Ok(Ok(ready)) => Ok(ready),
        Ok(Err(reason)) => Err(ServerSelectionError::TransportUnready(reason)),
        Err(_elapsed) => Err(ServerSelectionError::TransportUnready(format!(
            "no readiness within {}s",
            budget.as_secs()
        ))),
    }
}

/// Survey every candidate over the sweep exit concurrently, recording each
/// attempt in the indexer history like any probe.
async fn survey(
    socks5_addr: std::net::SocketAddr,
    candidates: &[Uri],
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
) -> Vec<SurveyResult> {
    let timeout = zingo_netutils::time::PROBE_LEG_TIMEOUT;
    let dial = crate::mixnet::Socks5Dial::of(socks5_addr);
    let dial = &dial;
    futures::future::join_all(candidates.iter().map(|uri| async move {
        let reported = probe_one(dial, uri, timeout, history).await;
        SurveyResult {
            uri: uri.clone(),
            reported,
        }
    }))
    .await
}

/// One candidate's survey: `GetLightdInfo` over the sweep exit, its success
/// mapped to the reported chain and height, any failure to `None`.
async fn probe_one(
    dial: &crate::mixnet::Socks5Dial,
    uri: &Uri,
    timeout: Duration,
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
) -> Option<ProbeSuccess> {
    use crate::lightclient::indexer_history::{
        AttemptKind, AttemptRoute, FailureKind, IndexerAttempt, now_unix_secs,
    };
    let host = crate::correspondent::Host::of_uri(uri);
    let result = zingo_netutils::get_lightd_info_via_socks5(dial.as_str(), uri, timeout).await;
    let (reported, outcome) = match &result {
        Ok(info) => (
            Some(ProbeSuccess {
                chain: info.chain_name.clone(),
                height: info.block_height,
            }),
            Ok(()),
        ),
        Err(error) => (None, Err(FailureKind::classify(&error.to_string()))),
    };
    history.record(&IndexerAttempt {
        unix_secs: now_unix_secs(),
        host,
        route: AttemptRoute::Mixnet,
        kind: AttemptKind::Probe,
        millis: 0,
        phase: result
            .as_ref()
            .err()
            .map(|error| crate::mixnet::charge_phase(&crate::mixnet::socks5_transmit_stage(error))),
        exit: None,
        outcome,
    });
    reported
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HYPOTHESIS: the judgment compares against the wire's chain
    /// vocabulary (`main`, `test`, `regtest`), never `ChainType`'s own
    /// rendering (`mainnet`). Falsified if the mapping drifts back to the
    /// Display form, which emptied a 17-of-17 mainnet cohort on 2026-08-06.
    #[test]
    fn the_judgment_speaks_the_wire_chain_vocabulary() {
        use zingo_common_components::protocol::ActivationHeights;

        assert_eq!(
            lightd_chain_name(&crate::config::ChainType::Mainnet),
            "main"
        );
        assert_eq!(
            lightd_chain_name(&crate::config::ChainType::Testnet),
            "test"
        );
        assert_eq!(
            lightd_chain_name(&crate::config::ChainType::Regtest(
                ActivationHeights::default()
            )),
            "regtest"
        );
        assert_ne!(
            lightd_chain_name(&crate::config::ChainType::Mainnet),
            crate::config::ChainType::Mainnet.to_string(),
            "the Display form is not the wire form"
        );
    }
}

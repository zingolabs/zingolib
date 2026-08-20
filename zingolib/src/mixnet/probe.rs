//! Connectivity probes: the mixnet indexer probe and the staged sync-path
//! probe.
//!
//! The mixnet probe runs `GetLightdInfo` against an indexer through the
//! session's SOCKS5 proxy: it establishes an indexer's liveness over the
//! mixnet, the precondition every sync attach requires (the 2026-08-06
//! ruling), and its outcomes are appended to the cross-session indexer
//! history like send attempts, so reliability accumulates. The probe has
//! no clearnet leg: a session's only clearnet communication is sync
//! itself.
//!
//! The staged sync-path probe ([`probe_sync_server`]) serves connectivity
//! triage for the ordinary synchronization path — the sole clearnet
//! exception (the Connection Doctor, zingo-mobile's diagnostics plan): it
//! walks one server through TCP connect, secure-channel establishment,
//! and a `GetLightdInfo` round trip, timing each stage and reporting each
//! failure as a typed [`NetOpFailure`], so a non-developer's report says
//! which layer broke without anyone parsing prose. It contacts the server
//! from the client's real IP, so it is a user-invoked diagnostic, never
//! an automatic path.
//!
//! Every outcome in this module is data: success carries the server's chain
//! name and height as fields ([`ProbeSuccess`]), failure carries the
//! taxonomy record with its cause chain as a vector. Rendering belongs to
//! consumers.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use http::Uri;
use zingo_net_diag::{NetOpFailure, NetOpStage};
use zingo_netutils::{GetClientError, GrpcIndexer, Indexer as _};

use crate::lightclient::indexer_history::{
    AttemptKind, AttemptRoute, FailureKind, IndexerAttempt, IndexerHistoryHandle, now_unix_secs,
};

/// What a successful `GetLightdInfo` proves, as fields rather than a
/// formatted sentence, so the mobile FFI crosses it as data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeSuccess {
    /// The chain name the server reports (`main`, `test`, `regtest`).
    pub chain: String,
    /// The block height the server reports.
    pub height: u64,
}

impl ProbeSuccess {
    fn of(info: &zingo_netutils::lightwallet_protocol::LightdInfo) -> Self {
        ProbeSuccess {
            chain: info.chain_name.clone(),
            height: info.block_height,
        }
    }
}

/// One probe attempt's outcome and timing.
#[derive(Clone, Debug)]
pub struct ProbeLeg {
    /// The server's identity on success, or the typed failure record.
    pub outcome: Result<ProbeSuccess, NetOpFailure>,
    /// How long the leg took, in milliseconds.
    pub millis: u64,
}

/// One indexer's liveness probe over the mixnet route.
#[derive(Clone, Debug)]
pub struct MixnetProbe {
    /// The probed indexer's host.
    pub host: crate::correspondent::Host,
    /// The probe's outcome through the session's SOCKS5 proxy.
    pub leg: ProbeLeg,
}

/// The stage of [`GetClientError`] by typed match: a bad URI never touched
/// the network, and tonic reports its whole connect phase (DNS, TCP, TLS)
/// as one transport failure, which lands on [`NetOpStage::RemoteConnect`];
/// the staged probe separates those phases where the distinction matters.
fn get_client_stage(error: &GetClientError) -> NetOpStage {
    match error {
        GetClientError::InvalidScheme | GetClientError::InvalidAuthority => {
            NetOpStage::RouteResolution
        }
        GetClientError::Transport(_) => NetOpStage::RemoteConnect,
    }
}

/// Probes `indexer` through the SOCKS5 proxy, recording the attempt.
async fn mixnet_leg(
    socks5_addr: std::net::SocketAddr,
    indexer: &Uri,
    timeout: Duration,
    history: &IndexerHistoryHandle,
    host: &crate::correspondent::Host,
) -> ProbeLeg {
    let started = Instant::now();
    let outcome = zingo_netutils::Socks5Indexer::new(socks5_addr, indexer.clone(), timeout)
        .get_lightd_info()
        .await
        .map(|info| ProbeSuccess::of(&info))
        .map_err(|e| super::socks5_transmit_failure(&e, host));
    let leg = ProbeLeg {
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        outcome,
    };
    record_probe(history, host, AttemptRoute::Mixnet, &leg);
    leg
}

fn record_probe(
    history: &IndexerHistoryHandle,
    host: &crate::correspondent::Host,
    route: AttemptRoute,
    leg: &ProbeLeg,
) {
    history.record(&IndexerAttempt {
        unix_secs: now_unix_secs(),
        host: host.clone(),
        route,
        kind: AttemptKind::Probe,
        millis: leg.millis,
        phase: leg
            .outcome
            .as_ref()
            .err()
            .map(|failure| crate::mixnet::charge_phase(&failure.stage)),
        outcome: match &leg.outcome {
            Ok(_) => Ok(()),
            // The history store is a pre-existing rendered-text seam
            // (FailureKind classifies its own coarse buckets); the typed
            // record stays with the leg.
            Err(failure) => Err(FailureKind::classify(&failure.to_string())),
        },
    });
}

/// Whether `indexer` may be probed: `https` on port 443, the one endpoint
/// shape the mixnet exit policy carries.
pub fn probe_eligible(indexer: &Uri) -> bool {
    indexer.scheme_str() == Some("https") && indexer.port_u16().unwrap_or(443) == 443
}

/// Runs the liveness probe against one indexer over the mixnet route.
pub(crate) async fn probe_indexer(
    indexer: &Uri,
    socks5_addr: std::net::SocketAddr,
    timeout: Duration,
    history: &IndexerHistoryHandle,
) -> MixnetProbe {
    let host = crate::correspondent::Host::of_uri(indexer);
    let leg = mixnet_leg(socks5_addr, indexer, timeout, history, &host).await;
    MixnetProbe { host, leg }
}

/// One step of the staged sync-path probe, named by what it establishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncProbeStep {
    /// A raw TCP connection to the server's host and port.
    TcpConnect,
    /// The secure channel on top of a proven TCP path: TLS and the HTTP/2
    /// session ride this one step, because the transport establishes them
    /// as one connect phase. With [`Self::TcpConnect`] already green, a
    /// failure here is the secure channel, not reachability.
    TlsChannel,
    /// A `GetLightdInfo` round trip over the established channel.
    GrpcInfo,
}

impl std::fmt::Display for SyncProbeStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncProbeStep::TcpConnect => write!(f, "tcp-connect"),
            SyncProbeStep::TlsChannel => write!(f, "tls-channel"),
            SyncProbeStep::GrpcInfo => write!(f, "grpc-info"),
        }
    }
}

/// One timed stage of the staged sync-path probe.
#[derive(Clone, Debug)]
pub struct SyncProbeStage {
    /// Which step ran.
    pub step: SyncProbeStep,
    /// How long the step took, in milliseconds.
    pub millis: u64,
    /// The typed failure, or `None` when the step passed.
    pub failure: Option<NetOpFailure>,
}

/// The staged sync-path probe's report for one server.
#[derive(Clone, Debug)]
pub struct SyncServerProbe {
    /// The probed server's host.
    pub server: String,
    /// The stages that ran, in order. The run stops at the first failure,
    /// so a stage that does not appear was never reached.
    pub stages: Vec<SyncProbeStage>,
    /// The server's identity when every stage passed.
    pub info: Option<ProbeSuccess>,
}

/// The port a probe dials when the URI names none: the scheme's default.
fn probe_port(server: &Uri) -> u16 {
    server.port_u16().unwrap_or_else(|| {
        if server.scheme_str() == Some("http") {
            80
        } else {
            443
        }
    })
}

/// Walks `server` through the staged sync-path probe: TCP connect, the
/// secure channel, and a `GetLightdInfo` round trip, each stage bounded by
/// `stage_timeout` and timed, each failure a typed [`NetOpFailure`]. Runs
/// against the ordinary synchronization path (no tunnel, no wallet, no
/// lock), so the Connection Doctor can call it per configured server.
pub async fn probe_sync_server(server: &Uri, stage_timeout: Duration) -> SyncServerProbe {
    let host = server
        .host()
        .map_or_else(|| server.to_string(), str::to_string);
    let target = format!("{host}:{}", probe_port(server));
    let mut stages = Vec::new();
    let timed_out = || {
        NetOpFailure::message(
            NetOpStage::TimedOut {
                after_ms: stage_timeout.as_millis().try_into().unwrap_or(u64::MAX),
            },
            &target,
            format!("no answer within {stage_timeout:.0?}"),
        )
    };

    // Stage 1: raw TCP proves reachability apart from everything above it.
    let started = Instant::now();
    let tcp = tokio::time::timeout(
        stage_timeout,
        tokio::net::TcpStream::connect((host.as_str(), probe_port(server))),
    )
    .await;
    let failure = match &tcp {
        Ok(Ok(_)) => None,
        Ok(Err(io)) => Some(NetOpFailure::from_error(
            NetOpStage::RemoteConnect,
            &target,
            io,
        )),
        Err(_) => Some(timed_out()),
    };
    let failed = failure.is_some();
    stages.push(SyncProbeStage {
        step: SyncProbeStep::TcpConnect,
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        failure,
    });
    if failed {
        return SyncServerProbe {
            server: host,
            stages,
            info: None,
        };
    }

    // Stage 2: the secure channel. TCP is proven, so a transport failure
    // here is TLS (or the HTTP/2 session riding its establishment).
    let started = Instant::now();
    let channel = tokio::time::timeout(stage_timeout, GrpcIndexer::new(server.clone())).await;
    let (failure, grpc) = match channel {
        Ok(Ok(grpc)) => (None, Some(grpc)),
        Ok(Err(e)) => {
            let stage = match get_client_stage(&e) {
                NetOpStage::RemoteConnect => NetOpStage::RemoteTls,
                other => other,
            };
            (Some(NetOpFailure::from_error(stage, &target, &e)), None)
        }
        Err(_) => (Some(timed_out()), None),
    };
    stages.push(SyncProbeStage {
        step: SyncProbeStep::TlsChannel,
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        failure,
    });
    let Some(mut grpc) = grpc else {
        return SyncServerProbe {
            server: host,
            stages,
            info: None,
        };
    };

    // Stage 3: the RPC itself.
    let started = Instant::now();
    let rpc = tokio::time::timeout(stage_timeout, grpc.get_lightd_info(stage_timeout)).await;
    let (failure, info) = match rpc {
        Ok(Ok(info)) => (None, Some(ProbeSuccess::of(&info))),
        Ok(Err(status)) => (
            Some(NetOpFailure::from_error(
                NetOpStage::RemoteHttp,
                &target,
                &status,
            )),
            None,
        ),
        Err(_) => (Some(timed_out()), None),
    };
    stages.push(SyncProbeStage {
        step: SyncProbeStep::GrpcInfo,
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        failure,
    });
    SyncServerProbe {
        server: host,
        stages,
        info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use zingo_netutils::time::test::FAST_STAGE_BOUND;

    fn uri(text: &str) -> Uri {
        text.parse().expect("static uri")
    }

    /// A closed port fails at the first stage as a typed remote-connect
    /// failure, and no later stage runs.
    #[tokio::test]
    async fn a_closed_port_fails_the_tcp_stage_and_stops() {
        // Bind then drop, so the port is closed when the probe dials it.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let probe = probe_sync_server(&uri(&format!("https://{closed}")), FAST_STAGE_BOUND).await;
        assert_eq!(probe.stages.len(), 1, "the run stops at the first failure");
        assert_eq!(probe.stages[0].step, SyncProbeStep::TcpConnect);
        let failure = probe.stages[0].failure.as_ref().expect("stage failed");
        assert_eq!(failure.stage, NetOpStage::RemoteConnect);
        assert!(probe.info.is_none());
    }

    /// A listener that accepts TCP but never speaks TLS passes the first
    /// stage and fails the second within the bound — the staged separation
    /// that a single connect phase cannot report.
    #[tokio::test]
    async fn a_silent_listener_passes_tcp_and_fails_the_channel_stage() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            loop {
                let Ok((_sock, _)) = listener.accept().await else {
                    return;
                };
                // Accept and hold; never answer.
            }
        });

        let probe = probe_sync_server(&uri(&format!("https://{addr}")), FAST_STAGE_BOUND).await;
        assert_eq!(probe.stages.len(), 2);
        assert!(probe.stages[0].failure.is_none(), "TCP is reachable");
        let failure = probe.stages[1].failure.as_ref().expect("channel must fail");
        assert!(
            matches!(
                failure.stage,
                NetOpStage::RemoteTls | NetOpStage::TimedOut { .. }
            ),
            "a non-TLS listener fails the channel stage as TLS or a bound: {failure}"
        );
        assert!(probe.info.is_none());
        hold.abort();
    }

    /// A URI without a port dials the scheme default.
    #[test]
    fn the_probe_port_falls_back_to_the_scheme_default() {
        assert_eq!(probe_port(&uri("https://zec.rocks")), 443);
        assert_eq!(probe_port(&uri("http://localhost")), 80);
        assert_eq!(probe_port(&uri("https://zec.rocks:9067")), 9067);
    }

    /// Live staged probe against a public indexer; run by hand.
    #[tokio::test]
    #[ignore = "contacts a live public indexer over clearnet"]
    async fn live_staged_probe_smoke() {
        let probe = probe_sync_server(
            &uri("https://zec.rocks:443"),
            zingo_netutils::time::test::LIVE_STAGE_BOUND,
        )
        .await;
        assert_eq!(probe.stages.len(), 3, "all stages ran: {probe:?}");
        assert!(probe.info.is_some(), "a live indexer answers: {probe:?}");
    }
}

//! Traffic and state observability across the test pipeline:
//! wallet ↔ zainod ↔ zebrad ↔ harness.
//!
//! HYPOTHESIS (this module exists to adjudicate it): a regtest
//! Validator's chain mutates only via its RPC surface — peer lists are
//! empty, no internal miner runs, and the Indexer only reads. If that
//! holds, complete observation of chain *effects* plus a ledger of the
//! RPC calls this crate *issues* attributes every mutation: a tip event
//! with no matching ledger entry is foreign RPC traffic, by
//! elimination. If it does not hold — the chain moves while every tap
//! and the ledger are silent — the hypothesis is disproven and the
//! instrument has caught something bigger than a port collision.
//!
//! Two instruments compose the observatory:
//!
//! - [`StateWatch`] polls an [`Observable`] node's externally visible
//!   state fingerprint and records every transition. Implementations
//!   exist for all three pipeline nodes: [`ZebradState`] (tip + peer
//!   count), [`ZainodState`] (indexed tip), and [`WalletState`] (synced
//!   height).
//! - [`LinkTap`] is an interposable TCP relay that records the traffic
//!   crossing one hop. The wallet→zainod and harness→zebrad hops are
//!   wireable from this crate (we mint both endpoints); the
//!   zainod→zebrad hop is NOT — `LocalNet::launch` overwrites
//!   `ZainodConfig::validator_port` with the real validator port, so
//!   interposing there needs a `zcash_local_net` addition
//!   (`LocalNet::from_parts` or a port override). Foreign clients dial
//!   zebra's listener directly and no userspace tap can see them;
//!   their *effects* are what [`StateWatch`] captures.
//!
//! This module is deliberately prototyped here rather than in
//! `zcash_local_net`: iteration against live failures beats pinning a
//! new infra revision per tweak. The trait vocabulary mirrors the infra
//! crate's process model so the proven design can migrate there, where
//! process wiring (and the missing zainod→zebrad hop) natively lives.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zingo_netutils::Indexer as _;
use zingolib::lightclient::DEFAULT_REQUEST_TIMEOUT;
use zingolib::wallet::LightWallet;

use crate::validator_rpc;

/// A pipeline node whose externally observable state can be polled.
/// Implementors are cheap handles (a port, a URI, a wallet reference),
/// not the nodes themselves.
pub trait Observable: Send + 'static {
    /// The node's name in reports.
    const NODE: &'static str;

    /// How often [`StateWatch`] polls this node.
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    /// One poll of the node's state fingerprint. `None` when the node
    /// is unreachable (launch and teardown windows are normal); the
    /// watch polls through it.
    fn fingerprint(&self) -> impl std::future::Future<Output = Option<String>> + Send;
}

/// One observed change of an [`Observable`]'s fingerprint.
#[derive(Clone, Debug)]
pub struct StateEvent {
    /// Time since the watch was armed.
    pub at: Duration,
    /// The fingerprint observed.
    pub fingerprint: String,
}

/// Records every transition of an [`Observable`]'s fingerprint until
/// dropped. The full timeline lands in the per-test observatory log at
/// teardown (see `setup_metrics`); assertion messages embed it via
/// [`StateWatch::render`].
pub struct StateWatch<O: Observable> {
    events: Arc<Mutex<Vec<StateEvent>>>,
    armed: Instant,
    task: tokio::task::JoinHandle<()>,
    _observer: std::marker::PhantomData<O>,
}

impl<O: Observable> StateWatch<O> {
    /// Arm the watch: spawn the poll task and start recording.
    pub fn arm(observer: O) -> Self {
        let events: Arc<Mutex<Vec<StateEvent>>> = Arc::default();
        let armed = Instant::now();
        let recorder = events.clone();
        let task = tokio::spawn(async move {
            let mut last: Option<String> = None;
            loop {
                if let Some(fingerprint) = observer.fingerprint().await
                    && last.as_deref() != Some(&fingerprint)
                {
                    last = Some(fingerprint.clone());
                    recorder.lock().unwrap().push(StateEvent {
                        at: armed.elapsed(),
                        fingerprint,
                    });
                }
                tokio::time::sleep(O::POLL_INTERVAL).await;
            }
        });
        StateWatch {
            events,
            armed,
            task,
            _observer: std::marker::PhantomData,
        }
    }

    /// The transition timeline observed so far.
    pub fn events(&self) -> Vec<StateEvent> {
        self.events.lock().unwrap().clone()
    }

    /// The most recent observation, if any poll has succeeded yet.
    pub fn last(&self) -> Option<StateEvent> {
        self.events.lock().unwrap().last().cloned()
    }

    /// Seconds since the watch was armed, for correlating external
    /// timestamps with [`StateEvent::at`].
    pub fn elapsed(&self) -> Duration {
        self.armed.elapsed()
    }

    /// Render the timeline for assertion messages and the observatory
    /// log file.
    pub fn render(&self) -> String {
        let events = self.events.lock().unwrap();
        if events.is_empty() {
            return format!("  ({} not yet observed)", O::NODE);
        }
        events
            .iter()
            .map(|e| {
                format!(
                    "  {:>8.3}s {} {}",
                    e.at.as_secs_f64(),
                    O::NODE,
                    e.fingerprint
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One line for the teardown summary: transition count and the
    /// final fingerprint.
    pub fn summary(&self) -> String {
        let events = self.events.lock().unwrap();
        match events.last() {
            None => format!("{}: never observed", O::NODE),
            Some(last) => format!(
                "{}: {} transitions, final at {:.3}s: {}",
                O::NODE,
                events.len(),
                last.at.as_secs_f64(),
                last.fingerprint
            ),
        }
    }
}

impl<O: Observable> Drop for StateWatch<O> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The Validator's externally observable chain state: best height, best
/// block hash, and connected peer count. A nonzero peer count on
/// regtest is itself a finding — it names a mutation channel the
/// isolation assumptions say cannot exist.
pub struct ZebradState {
    /// The Validator's JSON-RPC port (the real one, not a tap: watch
    /// polls stay out of the tap record).
    pub rpc_port: u16,
}

impl Observable for ZebradState {
    const NODE: &'static str = "zebrad";

    async fn fingerprint(&self) -> Option<String> {
        let (height, hash) = validator_rpc::try_get_chain_info(self.rpc_port).await?;
        let peers = validator_rpc::try_get_peer_count(self.rpc_port).await?;
        Some(format!("height {height} tip {hash} peers {peers}"))
    }
}

/// The Indexer's externally observable state: the tip its chain index
/// serves. Lag behind [`ZebradState`] is normal (poll-based ingestion);
/// a tip zebrad never had is not.
pub struct ZainodState {
    /// The Indexer's gRPC URI (the real one, not a tap).
    pub uri: http::Uri,
}

impl Observable for ZainodState {
    const NODE: &'static str = "zainod";
    const POLL_INTERVAL: Duration = Duration::from_millis(250);

    async fn fingerprint(&self) -> Option<String> {
        let mut indexer = zingo_netutils::GrpcIndexer::new(self.uri.clone())
            .await
            .ok()?;
        let latest = indexer
            .get_latest_block(DEFAULT_REQUEST_TIMEOUT)
            .await
            .ok()?;
        Some(format!("height {}", latest.height))
    }
}

/// A wallet's externally observable state: its synced height. Arm one
/// per wallet under scrutiny; scenario constructors do not arm these
/// automatically because wallets outlive and outnumber the net handle.
pub struct WalletState {
    /// Name in reports (e.g. "faucet", "recipient").
    pub label: &'static str,
    /// Shared handle to the wallet under observation.
    pub wallet: Arc<tokio::sync::RwLock<LightWallet>>,
}

impl Observable for WalletState {
    const NODE: &'static str = "wallet";
    const POLL_INTERVAL: Duration = Duration::from_millis(250);

    async fn fingerprint(&self) -> Option<String> {
        let wallet = self.wallet.read().await;
        let synced = wallet.wallet_blocks.keys().max().copied();
        Some(format!("{} synced {synced:?}", self.label))
    }
}

/// One recorded traffic chunk crossing a tapped hop.
#[derive(Clone, Debug)]
pub struct TapEvent {
    /// Time since the tap opened.
    pub at: Duration,
    /// Which accepted connection the chunk belongs to.
    pub connection: usize,
    /// `->` toward the upstream, `<-` back toward the client.
    pub direction: &'static str,
    /// Chunk size in bytes.
    pub bytes: usize,
    /// Best-effort content note: the JSON-RPC method name when the
    /// chunk carries one (`"method":"..."`), empty otherwise (gRPC
    /// frames are recorded by size and timing only).
    pub note: String,
}

/// An interposable TCP relay recording all traffic on one hop. Clients
/// dial [`LinkTap::port`]; the tap forwards to the upstream port and
/// records every chunk in both directions. The full record lands in
/// the per-test observatory log at teardown (see `setup_metrics`).
pub struct LinkTap {
    label: &'static str,
    port: u16,
    events: Arc<Mutex<Vec<TapEvent>>>,
    task: tokio::task::JoinHandle<()>,
}

impl LinkTap {
    /// Open a tap in front of `127.0.0.1:upstream_port`. Binding to
    /// port 0 makes the OS pick a genuinely unused port — no
    /// check-then-bind race.
    pub async fn open(label: &'static str, upstream_port: u16) -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("tap listener must bind");
        let port = listener
            .local_addr()
            .expect("bound listener has an addr")
            .port();
        let events: Arc<Mutex<Vec<TapEvent>>> = Arc::default();
        let opened = Instant::now();
        let recorder = events.clone();
        let task = tokio::spawn(async move {
            let connections = AtomicUsize::new(0);
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    continue;
                };
                let connection = connections.fetch_add(1, Ordering::Relaxed);
                let Ok(upstream) =
                    tokio::net::TcpStream::connect(("127.0.0.1", upstream_port)).await
                else {
                    continue;
                };
                let (client_read, client_write) = client.into_split();
                let (upstream_read, upstream_write) = upstream.into_split();
                tokio::spawn(relay(
                    client_read,
                    upstream_write,
                    recorder.clone(),
                    opened,
                    connection,
                    "->",
                ));
                tokio::spawn(relay(
                    upstream_read,
                    client_write,
                    recorder.clone(),
                    opened,
                    connection,
                    "<-",
                ));
            }
        });
        LinkTap {
            label,
            port,
            events,
            task,
        }
    }

    /// The port clients dial to cross this hop through the tap.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The traffic record so far.
    pub fn events(&self) -> Vec<TapEvent> {
        self.events.lock().unwrap().clone()
    }

    /// Render the record for assertion messages and the observatory
    /// log file.
    pub fn render(&self) -> String {
        let events = self.events.lock().unwrap();
        if events.is_empty() {
            return format!("  ({}: no traffic)", self.label);
        }
        events
            .iter()
            .map(|e| {
                format!(
                    "  {:>8.3}s conn{} {} {:>5}B {}",
                    e.at.as_secs_f64(),
                    e.connection,
                    e.direction,
                    e.bytes,
                    e.note
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One line for the teardown summary: connection and chunk counts,
    /// byte totals per direction, and how many chain-mutating calls
    /// crossed the hop.
    pub fn summary(&self) -> String {
        let events = self.events.lock().unwrap();
        if events.is_empty() {
            return format!("{}: no traffic", self.label);
        }
        let connections = events.iter().map(|e| e.connection).max().unwrap_or(0) + 1;
        let sent: usize = events
            .iter()
            .filter(|e| e.direction == "->")
            .map(|e| e.bytes)
            .sum();
        let received: usize = events
            .iter()
            .filter(|e| e.direction == "<-")
            .map(|e| e.bytes)
            .sum();
        let writes = events
            .iter()
            .filter(|e| crate::validator_rpc::is_write_method(&e.note))
            .count();
        format!(
            "{}: {} conns, {} chunks, {}B ->, {}B <-, {} chain-mutating calls",
            self.label,
            connections,
            events.len(),
            sent,
            received,
            writes
        )
    }
}

impl Drop for LinkTap {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Pump one direction of a tapped connection, recording each chunk.
async fn relay(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    recorder: Arc<Mutex<Vec<TapEvent>>>,
    opened: Instant,
    connection: usize,
    direction: &'static str,
) {
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let Ok(n) = from.read(&mut buffer).await else {
            break;
        };
        if n == 0 {
            break;
        }
        recorder.lock().unwrap().push(TapEvent {
            at: opened.elapsed(),
            connection,
            direction,
            bytes: n,
            note: json_rpc_method_note(&buffer[..n]),
        });
        if to.write_all(&buffer[..n]).await.is_err() {
            break;
        }
    }
}

/// Best-effort extraction of a JSON-RPC method name from a chunk.
fn json_rpc_method_note(chunk: &[u8]) -> String {
    let Ok(text) = std::str::from_utf8(chunk) else {
        return String::new();
    };
    let Some(after) = text.split(r#""method":"#).nth(1) else {
        return String::new();
    };
    after
        .split('"')
        .nth(1)
        .map(|method| method.to_string())
        .unwrap_or_default()
}

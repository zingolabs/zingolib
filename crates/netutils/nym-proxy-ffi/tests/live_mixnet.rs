#![cfg(feature = "live-mixnet")]
#![forbid(unsafe_code)]

//! Live mixnet start through the crate's own public API. It reaches the real
//! Nym network. The `live-mixnet` feature builds it. The default `cargo test`
//! does not.

use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zingo_nym_proxy_ffi::{
    DISCONNECT_BOUND, MixnetProxyHandle, ProxyDeathObserver, ProxyDeathReason,
    RUNTIME_DISPOSAL_GRACE, Socks5Endpoint,
};

/// The wall-clock budget for bringing up a proxy against the live mixnet.
const START_BOUND: Duration = Duration::from_secs(180);

/// The wall-clock budget for the ordered disconnect against the live mixnet.
const TEARDOWN_BOUND: Duration = Duration::from_secs(90);

/// Tests that the observation bound sits strictly outside the disconnect
/// bound plus the disposal grace, never equal to either.
#[test]
fn the_teardown_bound_nests_outside_the_production_bounds() {
    assert!(
        TEARDOWN_BOUND > DISCONNECT_BOUND + RUNTIME_DISPOSAL_GRACE,
        "an observation bound equal to the production bound is a coin flip"
    );
}

/// Records every death report behind a handle the test keeps after the box crosses into `start`.
struct RecordingObserver {
    deaths: Arc<Mutex<Vec<ProxyDeathReason>>>,
}

impl ProxyDeathObserver for RecordingObserver {
    fn on_death(&self, reason: ProxyDeathReason) {
        self.deaths.lock().unwrap().push(reason);
    }
}

/// One no-auth SOCKS5 method selection against the endpoint.
async fn socks5_no_auth_handshake(endpoint: &Socks5Endpoint) -> std::io::Result<[u8; 2]> {
    let mut stream =
        tokio::net::TcpStream::connect((endpoint.host.as_str(), endpoint.port)).await?;
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut reply = [0u8; 2];
    stream.read_exact(&mut reply).await?;
    Ok(reply)
}

/// One SOCKS5 session taken past its CONNECT, which is where the mixnet client
/// begins tracking the connection with its controller.
async fn socks5_connect(
    endpoint: &Socks5Endpoint,
    host: &str,
    port: u16,
) -> Result<tokio_socks::tcp::Socks5Stream<tokio::net::TcpStream>, tokio_socks::Error> {
    tokio_socks::tcp::Socks5Stream::connect((endpoint.host.as_str(), endpoint.port), (host, port))
        .await
}

/// Starts a live proxy off-thread, recording deaths, within the start budget.
fn start_live_proxy(deaths: &Arc<Mutex<Vec<ProxyDeathReason>>>) -> Arc<MixnetProxyHandle> {
    let observer = RecordingObserver {
        deaths: Arc::clone(deaths),
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(MixnetProxyHandle::start(Some(Box::new(observer))));
    });
    rx.recv_timeout(START_BOUND)
        .unwrap_or_else(|_| panic!("proxy start exceeded {}s", START_BOUND.as_secs()))
        .expect("proxy start against the live mixnet")
}

/// Tests that a handle dropped while a connection sits past its CONNECT tears
/// down without killing the process.
#[test]
fn live_proxy_survives_a_drop_while_a_connection_is_open() {
    // The abort this guards came from `SocksClient::drop`, which reports the
    // closing connection to a controller the same shutdown already took down
    // (nymtech/nym#7108). Observing the close with the process still alive is
    // the assertion, though the teardown tail past the close goes unobserved.
    let deaths = Arc::new(Mutex::new(Vec::new()));
    let handle = start_live_proxy(&deaths);

    let endpoint = handle.socks5_endpoint();
    let client = tokio::runtime::Runtime::new().expect("client runtime");
    let mut open = client
        .block_on(socks5_connect(&endpoint, "example.com", 80))
        .expect("SOCKS5 CONNECT against the live listener");

    // No `stop()` here. The drop must run the ordered disconnect by itself,
    // and `open` stays in scope so the connection is still there when it does.
    drop(handle);

    // The teardown thread's disconnect closes the SOCKS session, so within
    // the bound the socket reports the close: zero bytes or a reset, never a
    // hang. `peer_addr` cannot see a half-closed socket and proves nothing.
    let mut byte = [0u8; 1];
    let observed = client
        .block_on(async { tokio::time::timeout(TEARDOWN_BOUND, open.read(&mut byte)).await })
        .expect("teardown must close the SOCKS session within the bound");
    match observed {
        Ok(0) | Err(_) => {}
        Ok(unsolicited) => panic!("{unsolicited} unsolicited bytes instead of a close"),
    }
    assert!(
        deaths.lock().unwrap().is_empty(),
        "a deliberate drop must not report a death"
    );
}

/// Tests the sequence both hosts actually take: stop with a connection open,
/// then release the handle.
#[test]
fn live_proxy_stops_with_a_connection_open_then_releases() {
    let deaths = Arc::new(Mutex::new(Vec::new()));
    let handle = start_live_proxy(&deaths);

    let endpoint = handle.socks5_endpoint();
    let client = tokio::runtime::Runtime::new().expect("client runtime");
    let mut open = client
        .block_on(socks5_connect(&endpoint, "example.com", 80))
        .expect("SOCKS5 CONNECT against the live listener");

    // stop() begins the ordered disconnect off-thread. The session closing
    // is the observable completion.
    handle.stop();
    let mut byte = [0u8; 1];
    let observed = client
        .block_on(async { tokio::time::timeout(TEARDOWN_BOUND, open.read(&mut byte)).await })
        .expect("stop must close the SOCKS session within the bound");
    match observed {
        Ok(0) | Err(_) => {}
        Ok(unsolicited) => panic!("{unsolicited} unsolicited bytes instead of a close"),
    }
    assert!(
        deaths.lock().unwrap().is_empty(),
        "a deliberate stop must not report a death"
    );
    drop(handle);
}

#[test]
fn live_proxy_starts_serves_socks5_and_stops() {
    let deaths = Arc::new(Mutex::new(Vec::new()));
    let handle = start_live_proxy(&deaths);

    let endpoint = handle.socks5_endpoint();
    assert_eq!(endpoint.host, "127.0.0.1");
    assert_ne!(endpoint.port, 0);
    let exit_node = handle
        .exit_node()
        .expect("a running proxy names its exit node");
    assert!(!exit_node.is_empty());

    let reply = tokio::runtime::Runtime::new()
        .expect("handshake runtime")
        .block_on(socks5_no_auth_handshake(&endpoint))
        .expect("SOCKS5 handshake against the live listener");
    assert_eq!(reply, [0x05, 0x00]);

    handle.stop();
    assert!(handle.exit_node().is_none());
    handle.stop();
    assert!(
        deaths.lock().unwrap().is_empty(),
        "a deliberate stop must not report a death"
    );
}

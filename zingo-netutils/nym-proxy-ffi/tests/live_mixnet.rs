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
    MixnetProxyHandle, ProxyDeathObserver, ProxyDeathReason, Socks5Endpoint,
};

/// The wall-clock budget for bringing up a proxy against the live mixnet.
const START_BOUND: Duration = Duration::from_secs(180);

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
) -> std::io::Result<tokio::net::TcpStream> {
    let mut stream =
        tokio::net::TcpStream::connect((endpoint.host.as_str(), endpoint.port)).await?;
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut selection = [0u8; 2];
    stream.read_exact(&mut selection).await?;

    let name_length = u8::try_from(host.len()).expect("test host name fits a SOCKS5 length byte");
    let mut request = vec![0x05, 0x01, 0x00, 0x03, name_length];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await?;

    // The client acknowledges a CONNECT with a fixed ten-byte success reply
    // carrying an IPv4 bound address, before it reaches the remote.
    let mut acknowledgement = [0u8; 10];
    stream.read_exact(&mut acknowledgement).await?;
    Ok(stream)
}

/// Tests that a handle dropped while a connection sits past its CONNECT tears
/// down without killing the process.
#[test]
fn live_proxy_survives_a_drop_while_a_connection_is_open() {
    // The abort this guards came from `SocksClient::drop`, which reports the
    // closing connection to a controller the same shutdown already took down
    // (nymtech/nym#7108). Reaching the last line at all is the assertion.
    let deaths = Arc::new(Mutex::new(Vec::new()));
    let observer = RecordingObserver {
        deaths: Arc::clone(&deaths),
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(MixnetProxyHandle::start(Some(Box::new(observer))));
    });
    let handle = rx
        .recv_timeout(START_BOUND)
        .unwrap_or_else(|_| panic!("proxy start exceeded {}s", START_BOUND.as_secs()))
        .expect("proxy start against the live mixnet");

    let endpoint = handle.socks5_endpoint();
    let client = tokio::runtime::Runtime::new().expect("client runtime");
    let open = client
        .block_on(socks5_connect(&endpoint, "example.com", 80))
        .expect("SOCKS5 CONNECT against the live listener");

    // No `stop()` here. The drop must run the ordered disconnect by itself,
    // and `open` stays in scope so the connection is still there when it does.
    drop(handle);

    assert!(
        open.peer_addr().is_ok(),
        "the client socket must outlive a teardown that never touched it"
    );
}

#[test]
fn live_proxy_starts_serves_socks5_and_stops_without_a_death_report() {
    let deaths = Arc::new(Mutex::new(Vec::new()));
    let observer = RecordingObserver {
        deaths: Arc::clone(&deaths),
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(MixnetProxyHandle::start(Some(Box::new(observer))));
    });
    let handle = rx
        .recv_timeout(START_BOUND)
        .unwrap_or_else(|_| panic!("proxy start exceeded {}s", START_BOUND.as_secs()))
        .expect("proxy start against the live mixnet");

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

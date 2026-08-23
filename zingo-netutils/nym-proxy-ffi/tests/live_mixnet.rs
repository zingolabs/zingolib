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

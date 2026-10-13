use std::net::Ipv4Addr;

use nym_sdk::{ShutdownToken, ShutdownTracker};
use nym_socks5_proxy_helpers::proxy_runner::ProxyRunner;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const CLOSED_INPUT_PANIC: &str = "BatchRealMessageReceiver has stopped receiving!";
const INPUT_CHANNEL_CAPACITY: usize = 1;
const PLAINTEXT_PER_MIX_PACKET: usize = 1024;
const CONNECTION_ID: u64 = 7;

/// - Binds a loopback TCP listener on an ephemeral port and connects to it.
async fn loopback_pair() -> std::io::Result<(TcpStream, TcpStream)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let (wallet_side, (proxy_side, _peer)) = tokio::try_join!(
        TcpStream::connect(listener.local_addr()?),
        listener.accept()
    )?;
    Ok((wallet_side, proxy_side))
}

async fn first_payload_after_keepalives(input_listener: &mut mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    loop {
        match input_listener.recv().await {
            Some(keepalive) if keepalive.is_empty() => continue,
            Some(payload) => return payload,
            None => panic!("the connection task holds a sender for as long as it runs"),
        }
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

/// - Binds a loopback TCP listener through `loopback_pair`.
/// - Spawns the SOCKS5 connection's inbound and outbound tasks.
#[tokio::test]
async fn a_socks5_connection_panics_when_the_input_listener_is_gone() -> std::io::Result<()> {
    let (mut wallet_side, proxy_side) = loopback_pair().await?;
    let (into_mixnet, mut input_listener) = mpsc::channel::<Vec<u8>>(INPUT_CHANNEL_CAPACITY);
    let (from_mixnet, from_mixnet_receiver) = futures::channel::mpsc::unbounded();
    let shutdown = ShutdownToken::new();
    let runner = ProxyRunner::new(
        proxy_side,
        "wallet".to_string(),
        "exit".to_string(),
        from_mixnet_receiver,
        into_mixnet,
        PLAINTEXT_PER_MIX_PACKET,
        CONNECTION_ID,
        None,
        ShutdownTracker::new_from_external_shutdown_token(shutdown.clone()),
    );
    let connection = tokio::spawn(runner.run(|socket_data| socket_data.data));

    wallet_side.write_all(b"first").await?;
    assert_eq!(
        first_payload_after_keepalives(&mut input_listener).await,
        b"first"
    );

    drop(input_listener);
    drop(from_mixnet);
    wallet_side.write_all(b"second").await?;

    let outcome = connection
        .await
        .expect("the test holds the only handle to the connection task and never aborts it");
    let join_error = match outcome {
        Err(join_error) => join_error,
        Ok(_runner) => panic!("the inbound task returned instead of panicking"),
    };
    assert!(join_error.is_panic(), "the inbound task was not aborted");
    let payload = join_error.into_panic();
    assert_eq!(panic_text(payload.as_ref()), Some(CLOSED_INPUT_PANIC));
    Ok(())
}

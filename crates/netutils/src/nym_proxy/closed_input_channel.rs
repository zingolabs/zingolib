use std::net::Ipv4Addr;
use std::time::Duration;

use nym_sdk::ShutdownTracker;
use nym_socks5_proxy_helpers::proxy_runner;
use tokio::io::AsyncWriteExt;
use tokio::net;
use tokio::sync::mpsc;

const CLOSED_INPUT_PANIC: &str = "BatchRealMessageReceiver has stopped receiving!";
const INPUT_CHANNEL_CAPACITY: usize = 1;
const PLAINTEXT_PER_MIX_PACKET: usize = 1024;
const CONNECTION_ID: u64 = 7;
const CONNECTION_EXIT_BOUND: Duration = Duration::from_secs(10);

/// - Binds a loopback TCP listener on an ephemeral port.
/// - Connects to that listener.
async fn loopback_pair() -> std::io::Result<(net::TcpStream, net::TcpStream)> {
    let listener = net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let (wallet_side, (proxy_side, _peer)) = tokio::try_join!(
        net::TcpStream::connect(listener.local_addr()?),
        listener.accept()
    )?;
    Ok((wallet_side, proxy_side))
}

/// - Panics when the channel closes before `expected` has arrived.
async fn receive_payload(input_listener: &mut mpsc::Receiver<Vec<u8>>, expected: &[u8]) {
    let mut received = Vec::new();
    while received.len() < expected.len() {
        match input_listener.recv().await {
            Some(chunk) => received.extend(chunk),
            None => panic!("the connection task holds a sender for as long as it runs"),
        }
    }
    assert_eq!(received, expected);
}

fn join_panic_message(payload: &(dyn std::any::Any + Send)) -> Option<&str> {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
}

/// - Binds a loopback TCP listener through `loopback_pair`.
/// - Spawns the SOCKS5 connection's inbound and outbound tasks.
/// - Panics when the inbound task survives the closed channel.
#[tokio::test]
async fn a_socks5_connection_panics_when_the_input_listener_is_gone() -> std::io::Result<()> {
    let (mut wallet_side, proxy_side) = loopback_pair().await?;
    let (into_mixnet, mut input_listener) = mpsc::channel::<Vec<u8>>(INPUT_CHANNEL_CAPACITY);
    let (from_mixnet, from_mixnet_receiver) = futures_channel::mpsc::unbounded();
    let runner = proxy_runner::ProxyRunner::new(
        proxy_side,
        "wallet".to_string(),
        "exit".to_string(),
        from_mixnet_receiver,
        into_mixnet,
        PLAINTEXT_PER_MIX_PACKET,
        CONNECTION_ID,
        None,
        ShutdownTracker::default(),
    );
    let connection = tokio::spawn(runner.run(|socket_data| socket_data.data));

    wallet_side.write_all(b"first").await?;
    receive_payload(&mut input_listener, b"first").await;

    drop(input_listener);
    wallet_side.write_all(b"second").await?;
    drop(from_mixnet);

    let outcome = tokio::time::timeout(CONNECTION_EXIT_BOUND, connection)
        .await
        .expect("the outbound half ends once its mixnet sender is dropped, so both halves exit")
        .expect("the test holds the only handle to the connection task and never aborts it");
    let join_error = match outcome {
        Err(join_error) => join_error,
        Ok(_runner) => panic!("the inbound task returned instead of panicking"),
    };
    assert!(join_error.is_panic(), "the inbound task was not aborted");
    let payload = join_error.into_panic();
    let message = join_panic_message(payload.as_ref());
    assert!(
        message.is_some(),
        "the panic payload is neither a str nor a String"
    );
    assert_eq!(message, Some(CLOSED_INPUT_PANIC));
    Ok(())
}

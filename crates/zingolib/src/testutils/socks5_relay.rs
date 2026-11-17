//! A SOCKS5 relay on loopback that stands in for a mixnet exit node.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

const SOCKS_VERSION: u8 = 0x05;
const METHOD_NO_AUTHENTICATION: u8 = 0x00;
const METHOD_NONE_ACCEPTABLE: u8 = 0xFF;
const COMMAND_CONNECT: u8 = 0x01;
const RESERVED: u8 = 0x00;
const ADDRESS_IPV4: u8 = 0x01;
const ADDRESS_DOMAIN: u8 = 0x03;
const ADDRESS_IPV6: u8 = 0x04;
const REPLY_SUCCEEDED: u8 = 0x00;
const REPLY_HOST_UNREACHABLE: u8 = 0x04;
const REPLY_CONNECTION_REFUSED: u8 = 0x05;
const REPLY_COMMAND_NOT_SUPPORTED: u8 = 0x07;
const REPLY_ADDRESS_NOT_SUPPORTED: u8 = 0x08;
const IPV4_LEN: usize = 4;
const IPV6_LEN: usize = 16;
const PORT_LEN: usize = 2;

/// A destination as the wallet names it: host and port.
pub type Destination = (String, u16);

/// A running relay.
pub struct Socks5Relay {
    addr: SocketAddr,
    routes: Arc<Mutex<HashMap<Destination, SocketAddr>>>,
    requested: Arc<Mutex<Vec<Destination>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Socks5Relay {
    /// Starts a relay on an ephemeral localhost port.
    pub async fn launch() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral localhost port binds");
        let addr = listener
            .local_addr()
            .expect("a bound socket has an address");
        let routes: Arc<Mutex<HashMap<Destination, SocketAddr>>> = Arc::default();
        let requested: Arc<Mutex<Vec<Destination>>> = Arc::default();
        let server = {
            let routes = Arc::clone(&routes);
            let requested = Arc::clone(&requested);
            tokio::spawn(async move {
                while let Ok((client, _)) = listener.accept().await {
                    let routes = Arc::clone(&routes);
                    let requested = Arc::clone(&requested);
                    tokio::spawn(async move {
                        let _ = relay(client, &routes, &requested).await;
                    });
                }
            })
        };
        Socks5Relay {
            addr,
            routes,
            requested,
            server,
        }
    }

    /// The relay's SOCKS5 address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Routes the destination `uri` names to `to`.
    pub fn route(&self, uri: &http::Uri, to: SocketAddr) {
        self.routes
            .lock()
            .expect("relay route mutex")
            .insert(destination_of(uri), to);
    }

    /// Every destination the wallet asked for, in arrival order.
    pub fn requested(&self) -> Vec<Destination> {
        self.requested.lock().expect("relay request mutex").clone()
    }
}

impl Drop for Socks5Relay {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The destination the wallet sends for `uri`.
pub fn destination_of(uri: &http::Uri) -> Destination {
    (
        uri.host().expect("a routed uri has a host").to_string(),
        uri.port_u16().expect("a routed uri names its port"),
    )
}

async fn reply(client: &mut TcpStream, code: u8) -> std::io::Result<()> {
    let unspecified = [0u8; IPV4_LEN + PORT_LEN];
    client
        .write_all(&[SOCKS_VERSION, code, RESERVED, ADDRESS_IPV4])
        .await?;
    client.write_all(&unspecified).await
}

async fn relay(
    mut client: TcpStream,
    routes: &Mutex<HashMap<Destination, SocketAddr>>,
    requested: &Mutex<Vec<Destination>>,
) -> std::io::Result<()> {
    let mut greeting = [0u8; 2];
    client.read_exact(&mut greeting).await?;
    let [version, method_count] = greeting;
    let mut methods = vec![0u8; usize::from(method_count)];
    client.read_exact(&mut methods).await?;
    if version != SOCKS_VERSION || !methods.contains(&METHOD_NO_AUTHENTICATION) {
        return client
            .write_all(&[SOCKS_VERSION, METHOD_NONE_ACCEPTABLE])
            .await;
    }
    client
        .write_all(&[SOCKS_VERSION, METHOD_NO_AUTHENTICATION])
        .await?;

    let mut request = [0u8; 4];
    client.read_exact(&mut request).await?;
    let [_, command, _, address_type] = request;
    if command != COMMAND_CONNECT {
        return reply(&mut client, REPLY_COMMAND_NOT_SUPPORTED).await;
    }
    let host = match address_type {
        ADDRESS_IPV4 => {
            let mut octets = [0u8; IPV4_LEN];
            client.read_exact(&mut octets).await?;
            Ipv4Addr::from(octets).to_string()
        }
        ADDRESS_IPV6 => {
            let mut octets = [0u8; IPV6_LEN];
            client.read_exact(&mut octets).await?;
            Ipv6Addr::from(octets).to_string()
        }
        ADDRESS_DOMAIN => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            client.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => return reply(&mut client, REPLY_ADDRESS_NOT_SUPPORTED).await,
    };
    let mut port = [0u8; PORT_LEN];
    client.read_exact(&mut port).await?;
    let destination = (host, u16::from_be_bytes(port));

    requested
        .lock()
        .expect("relay request mutex")
        .push(destination.clone());
    let route = routes
        .lock()
        .expect("relay route mutex")
        .get(&destination)
        .copied();
    let Some(target) = route else {
        return reply(&mut client, REPLY_HOST_UNREACHABLE).await;
    };
    let Ok(mut upstream) = TcpStream::connect(target).await else {
        return reply(&mut client, REPLY_CONNECTION_REFUSED).await;
    };
    reply(&mut client, REPLY_SUCCEEDED).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

use super::socks5_transmit_stage;
use zl_netutils::diag::NetOpStage;
use zl_netutils::{ProxyDialFailure, Socks5TransmitError, TunnelFailure};

/// Every variant classifies by type alone; fabricated values cover the
/// whole table.
#[test]
fn every_variant_classifies_by_type_alone() {
    let io = || std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
    let table: Vec<(Socks5TransmitError, NetOpStage)> = vec![
        (
            Socks5TransmitError::ProxyUnreachable {
                proxy: "127.0.0.1:1080".into(),
                elapsed: std::time::Duration::from_secs(1),
                source: ProxyDialFailure::Io(io()),
            },
            NetOpStage::LocalProxyConnect,
        ),
        (
            Socks5TransmitError::TunnelRefused {
                destination: "zec.rocks:443".into(),
                elapsed: std::time::Duration::from_secs(1),
                source: TunnelFailure::TimedOut,
            },
            NetOpStage::SocksHandshake,
        ),
        (
            Socks5TransmitError::TunnelTransport {
                destination: "zec.rocks:443".into(),
                detail: "tls handshake eof".into(),
                source: None,
            },
            NetOpStage::TunnelTransport,
        ),
        (
            Socks5TransmitError::Rpc {
                destination: "zec.rocks:443".into(),
                status: zl_netutils::Status::unavailable("overloaded"),
            },
            NetOpStage::RemoteHttp,
        ),
        (
            Socks5TransmitError::TimedOut {
                destination: "zec.rocks:443".into(),
                after: std::time::Duration::from_secs(25),
            },
            NetOpStage::TimedOut { after_ms: 25_000 },
        ),
        (
            Socks5TransmitError::InsecureScheme {
                indexer: "http://zec.rocks:9067".into(),
            },
            NetOpStage::RouteResolution,
        ),
    ];
    for (error, expected) in table {
        assert_eq!(socks5_transmit_stage(&error), expected, "error: {error}");
    }
}

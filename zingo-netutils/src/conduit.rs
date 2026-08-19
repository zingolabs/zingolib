//! The conduit a wallet dials the mixnet through.
#![forbid(unsafe_code)]

use std::net::SocketAddr;

/// Somewhere to send mixnet traffic, proven for the epoch (ADR 0046).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixnetConduit {
    socks5: SocketAddr,
}

impl MixnetConduit {
    /// Wraps a ready transport's published address.
    pub fn over(socks5: SocketAddr) -> Self {
        MixnetConduit { socks5 }
    }

    /// Dialing address, while the transport lives.
    // The wallet reads this only to hand it to a dialer that still takes a
    // bare address: zingo-price dials without depending on this crate, and
    // twelve zingolib signatures carry a SocketAddr. ADR 0046 wants the
    // conduit opaque, which needs those dialers to accept one instead.
    pub fn socks5(&self) -> SocketAddr {
        self.socks5
    }
}

//! The Sentinel probe: evidence that a bound Exit Node carries traffic.
//!
//! A proving birth answers the Sentinel before its client takes any work
//! (ADR 0044). The Sentinel is not Correspondable and is never eligible
//! for a cohort or a verdict: it exists so a birth can tell an exit that
//! carries nothing from indexers that will not answer. The request is the
//! shape its address ordinarily serves — a DNS lookup — so neither the
//! exit nor the destination observes anything unusual, and the name
//! looked up is a constant that names nothing the wallet is interested in.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// The address the Sentinel probes: a public resolver reliable enough that
/// its silence indicts the tunnel rather than itself.
pub const SENTINEL_HOST: &str = "1.1.1.1";

/// The port the Sentinel probes, where its address serves DNS.
pub const SENTINEL_PORT: u16 = 53;

/// The reply bytes a probe reads before it stops: enough to prove bytes
/// crossed, never enough to need parsing.
const SENTINEL_READ_BYTES: usize = 64;

/// The transaction identifier every Sentinel query carries, fixed because
/// the Sentinel never matches a reply to a request: any reply proves the
/// exit, whatever it says.
const SENTINEL_QUERY_ID: [u8; 2] = [0xAB, 0xCD];

/// What one Sentinel probe proves about the bound Exit Node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitEvidence {
    /// The destination replied, so the exit carried the round trip. The
    /// reply's content is irrelevant: only a live exit can deliver one.
    Answered {
        /// How long the round trip took, in milliseconds.
        millis: u64,
    },
    /// Nothing came back within the budget, so the exit carries nothing.
    Silent,
}

impl ExitEvidence {
    /// Whether this evidence proves the exit carries traffic.
    pub fn proves_the_exit(self) -> bool {
        matches!(self, ExitEvidence::Answered { .. })
    }
}

/// Reads one probe attempt as evidence about the exit, where any completed
/// round trip answers and every failure to complete one is silence.
pub fn evidence_of(read: Option<usize>, millis: u64) -> ExitEvidence {
    match read {
        Some(bytes) if bytes > 0 => ExitEvidence::Answered { millis },
        _ => ExitEvidence::Silent,
    }
}

/// Probes the Sentinel through `socks5_addr`, returning what the attempt
/// proves about the exit that tunnel is bound to.
pub async fn probe_sentinel(socks5_addr: SocketAddr, budget: Duration) -> ExitEvidence {
    let started = Instant::now();
    let attempt = tokio::time::timeout(budget, round_trip(socks5_addr)).await;
    let millis = started.elapsed().as_millis() as u64;
    evidence_of(attempt.ok().and_then(|read| read.ok()), millis)
}

/// Opens the tunnel, sends the query, and reads whatever comes back.
async fn round_trip(socks5_addr: SocketAddr) -> Result<usize, ()> {
    let mut tunnel =
        tokio_socks::tcp::Socks5Stream::connect(socks5_addr, (SENTINEL_HOST, SENTINEL_PORT))
            .await
            .map_err(|_| ())?;
    tunnel.write_all(&sentinel_query()).await.map_err(|_| ())?;
    let mut buffer = [0u8; SENTINEL_READ_BYTES];
    tunnel.read(&mut buffer).await.map_err(|_| ())
}

/// The query the Sentinel sends: an ordinary `A` lookup of `example.com`,
/// length-prefixed as DNS over TCP requires.
fn sentinel_query() -> Vec<u8> {
    let mut body = Vec::from(SENTINEL_QUERY_ID);
    // Standard query, recursion desired; one question, no other records.
    body.extend_from_slice(&[0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for label in ["example", "com"] {
        body.push(label.len() as u8);
        body.extend_from_slice(label.as_bytes());
    }
    // Root label, then type A in class IN.
    body.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x01]);
    let mut framed = Vec::from((body.len() as u16).to_be_bytes());
    framed.extend(body);
    framed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HYPOTHESIS: any completed round trip proves the exit, whatever the
    /// destination said, and every failure to complete one is silence.
    /// Falsified if an empty read counts as proof or a reply does not.
    #[test]
    fn only_a_completed_round_trip_proves_the_exit() {
        assert_eq!(
            evidence_of(Some(63), 900),
            ExitEvidence::Answered { millis: 900 }
        );
        assert!(evidence_of(Some(63), 900).proves_the_exit());
        assert_eq!(evidence_of(Some(0), 3500), ExitEvidence::Silent);
        assert_eq!(evidence_of(None, 3500), ExitEvidence::Silent);
        assert!(!evidence_of(None, 3500).proves_the_exit());
    }

    /// HYPOTHESIS: the query is an ordinary DNS lookup — length-prefixed,
    /// one question, naming a constant the wallet has no interest in — so
    /// the exit and the resolver both see traffic they see constantly.
    /// Falsified if the framing or the question section drifts.
    #[test]
    fn the_query_is_an_ordinary_dns_lookup() {
        let query = sentinel_query();
        let framed_len = u16::from_be_bytes([query[0], query[1]]) as usize;
        assert_eq!(framed_len, query.len() - 2, "the length prefix frames it");
        assert_eq!(&query[2..4], &SENTINEL_QUERY_ID, "the id opens the body");
        assert_eq!(&query[6..8], &[0x00, 0x01], "exactly one question");
        let name: Vec<u8> = query[14..].to_vec();
        assert_eq!(name[0], 7, "the first label is `example`");
        assert_eq!(&name[1..8], b"example");
        assert_eq!(name[8], 3, "the second label is `com`");
        assert_eq!(&name[9..12], b"com");
        assert_eq!(
            &name[12..],
            &[0x00, 0x00, 0x01, 0x00, 0x01],
            "an A record in IN"
        );
    }
}

//! The Mixnet Mode session driver's types (ADR 0024, decision 2): the
//! typed status snapshot the one shared subscription delivers, the
//! per-session start policy, and the provisioning strategy a session hands
//! the driver at its go-online moment.
//!
//! The driver itself is [`crate::lightclient::LightClient::start_mixnet_session`];
//! this module holds its vocabulary. Status reaches subscribers through one
//! session-level `tokio::sync::watch` channel: the supervisor's tasks
//! publish transport transitions directly, the wallet's slot methods publish
//! the slot states, and nothing polls. The channel's keep-only-latest
//! semantics are the publication-sequencing guard — a stale snapshot can
//! never overtake a newer one.
#![forbid(unsafe_code)]

use std::sync::Arc;

use crate::mixnet::{DeathReport, Indicator};

/// One observation of Mixnet Mode for subscribers: the mode plus the
/// evidence scoped to it. Every field beyond the mode is `None` outside
/// the one mode it belongs to, exactly as the pull accessors are gated.
///
/// This is the wire snapshot the neon boundary carries to zingo-pc, the
/// boundary-carrier consumer ADR 0024 sequences last. The serde form omits
/// every absent field, so the `mode` token alone discriminates and each state
/// carries only its own evidence. The wire is pinned by the `wire_contract`
/// golden test below, the same way [`Indicator`]'s five tokens are pinned in
/// `mode.rs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "RawMixnetStatus")]
pub struct MixnetStatus {
    /// The Mixnet Mode at this observation.
    pub mode: Indicator,
    /// The local SOCKS5 address, present exactly while ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socks5_addr: Option<std::net::SocketAddr>,
    /// The Exit Node identities the ready transport bound, present only
    /// while ready.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub exits: Vec<crate::mixnet::ExitNodeId>,
    /// The transport's latest bootstrap progress line, present only while
    /// bootstrapping, so a subscriber can narrate the connect race.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap_detail: Option<String>,
    /// The latched death read whole, present only while died.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub death: Option<DeathReport>,
}

/// The lifted form before mode-scoping, through whose mirror serde reads
/// the typed fields — whose own deserializers refuse a malformed address
/// or exit entry as suspicious, because producer and consumer are pinned
/// to one code revision — so that [`TryFrom`] can refuse any evidence
/// outside its one mode.
#[derive(serde::Deserialize)]
struct RawMixnetStatus {
    mode: Indicator,
    #[serde(default)]
    socks5_addr: Option<std::net::SocketAddr>,
    #[serde(default)]
    exits: Vec<crate::mixnet::ExitNodeId>,
    #[serde(default)]
    bootstrap_detail: Option<String>,
    #[serde(default)]
    death: Option<DeathReport>,
}

impl TryFrom<RawMixnetStatus> for MixnetStatus {
    type Error = String;

    fn try_from(raw: RawMixnetStatus) -> Result<Self, Self::Error> {
        fn stray(field: &str, mode: Indicator) -> String {
            format!("{field} is not evidence for mode {}", mode.as_str())
        }
        // Route evidence rides every routable mode: earned Ready and
        // stale-proven PreviouslyProvenThisEpoch, which routes the same.
        if raw.socks5_addr.is_some() && !raw.mode.is_ready() {
            return Err(stray("socks5_addr", raw.mode));
        }
        if !raw.exits.is_empty() && !raw.mode.is_ready() {
            return Err(stray("exits", raw.mode));
        }
        if raw.bootstrap_detail.is_some() && raw.mode != Indicator::Bootstrapping {
            return Err(stray("bootstrap_detail", raw.mode));
        }
        if raw.death.is_some() && raw.mode != Indicator::Died {
            return Err(stray("death", raw.mode));
        }
        Ok(MixnetStatus {
            mode: raw.mode,
            socks5_addr: raw.socks5_addr,
            exits: raw.exits,
            bootstrap_detail: raw.bootstrap_detail,
            death: raw.death,
        })
    }
}

impl MixnetStatus {
    /// The snapshot of a slot state that carries no transport — unattached
    /// or switched off — where every transport-scoped field is absent.
    pub(crate) fn slot_only(mode: Indicator) -> Self {
        MixnetStatus {
            mode,
            socks5_addr: None,
            exits: Vec::new(),
            bootstrap_detail: None,
            death: None,
        }
    }

    /// A status carrying only the evidence its mode offers — route fields
    /// ride the routable modes and the death rides `Died` alone — so every
    /// published shape satisfies the wire guard by construction.
    pub(crate) fn evidenced(
        mode: Indicator,
        socks5_addr: Option<std::net::SocketAddr>,
        exits: Vec<crate::mixnet::ExitNodeId>,
        death: Option<DeathReport>,
    ) -> Self {
        let routable = mode.is_ready();
        MixnetStatus {
            mode,
            socks5_addr: if routable { socks5_addr } else { None },
            exits: if routable { exits } else { Vec::new() },
            bootstrap_detail: None,
            death: if mode == Indicator::Died { death } else { None },
        }
    }
}

/// The shared sending half of the session's one status channel. The wallet
/// owns it for the session's life and clones it into each transport's
/// supervisor tasks, so successive transports (an enable after a disable)
/// publish into the same channel a subscriber holds.
pub(crate) type StatusPublisher = Arc<tokio::sync::watch::Sender<MixnetStatus>>;

/// A fresh session-level status channel, opened in the ground state:
/// unattached, which carries no online intent. Receivers come from
/// `subscribe()` on the sender, so only the publisher is kept.
pub(crate) fn status_publisher() -> StatusPublisher {
    let (sender, _initial_receiver) =
        tokio::sync::watch::channel(MixnetStatus::slot_only(Indicator::Unattached));
    Arc::new(sender)
}

/// How a session's go-online moment treats the mixnet (ADR 0024, consent
/// at start). A two-variant enum rather than a boolean so the parameter
/// says what the opt-out is: a per-session consent act, never a persisted
/// preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixnetStartPolicy {
    /// The default for every connected session: force Mixnet Mode on by
    /// provisioning the transport now, so the bootstrap overlaps sync and
    /// the mixnet-only surfaces are protected from the session's start.
    ForcedOn,
    /// The user's startup opt-out — the explicit act that reaches
    /// switched off, recording the same per-session clearnet consent as an
    /// in-session toggle-off. Never persisted.
    OptedOutThisSession,
}

/// How the transport comes to exist at the go-online moment — the one
/// dimension along which consumers legitimately differ (ADR 0011, ADR
/// 0024): a desktop session spawns the bundled binary, a mobile platform
/// that hosts the proxy itself hands the wallet an address to attach to.
#[derive(Debug)]
pub enum ProvisionStrategy<'a> {
    /// Spawn the bundled `nym-proxy` binary, resolving its path from the
    /// consumer's platform hints through the one precedence rule in
    /// [`crate::mixnet::provision`].
    Spawn(crate::mixnet::provision::SpawnHints<'a>),
    /// Attach to an already-running, mobile-platform-hosted SOCKS5 endpoint.
    Attach {
        /// The endpoint's socket address.
        socks5_addr: &'a str,
        /// The Exit Node identities the mobile platform host reports as bound.
        exits: &'a [crate::mixnet::ExitNodeId],
    },
}

/// The golden wire pins for the status snapshot (ADR 0024, the boundary-carrier
/// mint). Each state is pinned literally in both directions: serializing the
/// canonical value produces exactly the bytes, and those bytes lift back to the
/// value. A drift here is a breaking change to zingo-pc's neon boundary, the
/// same contract `mode.rs`'s `wire_contract` keeps for the five mode tokens.
#[cfg(test)]
mod wire_contract {
    use std::time::{Duration, UNIX_EPOCH};

    use zingo_net_diag::{NetOpFailure, NetOpStage};

    use super::MixnetStatus;
    use crate::mixnet::{DeathReport, Indicator};

    // A fixed latch moment (2025-07-30T18:26:40Z) so the death pins are
    // deterministic; the wire carries it as milliseconds since the epoch.
    fn fixed_at() -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_753_900_000_000)
    }

    fn pin(status: &MixnetStatus, json: &str) {
        assert_eq!(serde_json::to_string(status).unwrap(), json);
        assert_eq!(&serde_json::from_str::<MixnetStatus>(json).unwrap(), status);
    }

    /// HYPOTHESIS: producer and consumer are pinned to one code revision,
    /// so a malformed wire value is suspicious — a Ready snapshot whose
    /// address is not a socket address, or whose exits carry a blank
    /// entry, refuses to deserialize whole. Falsified if either form
    /// lifts.
    #[test]
    fn a_malformed_snapshot_is_suspicious_and_refuses() {
        for json in [
            r#"{"mode":"ready","socks5_addr":"localhost:1080"}"#,
            r#"{"mode":"ready","exits":[""]}"#,
        ] {
            serde_json::from_str::<MixnetStatus>(json)
                .expect_err("a malformed wire value must refuse as suspicious");
        }
    }

    #[test]
    fn unattached_carries_only_its_token() {
        pin(
            &MixnetStatus::slot_only(Indicator::Unattached),
            r#"{"mode":"unattached"}"#,
        );
    }

    #[test]
    fn switched_off_carries_only_its_token() {
        pin(
            &MixnetStatus::slot_only(Indicator::SwitchedOff),
            r#"{"mode":"switched_off"}"#,
        );
    }

    #[test]
    fn bootstrapping_carries_its_narration() {
        pin(
            &MixnetStatus {
                mode: Indicator::Bootstrapping,
                socks5_addr: None,
                exits: Vec::new(),
                bootstrap_detail: Some("connecting to gateway".into()),
                death: None,
            },
            r#"{"mode":"bootstrapping","bootstrap_detail":"connecting to gateway"}"#,
        );
    }

    #[test]
    fn ready_carries_its_socks5_addr() {
        pin(
            &MixnetStatus {
                mode: Indicator::Ready,
                socks5_addr: Some("127.0.0.1:1080".parse().expect("the test address parses")),
                exits: Vec::new(),
                bootstrap_detail: None,
                death: None,
            },
            r#"{"mode":"ready","socks5_addr":"127.0.0.1:1080"}"#,
        );
    }

    #[test]
    fn ready_carries_its_bound_exits() {
        pin(
            &MixnetStatus {
                mode: Indicator::Ready,
                socks5_addr: Some("127.0.0.1:1080".parse().expect("the test address parses")),
                exits: vec!["exit-alpha".into(), "exit-beta".into()],
                bootstrap_detail: None,
                death: None,
            },
            r#"{"mode":"ready","socks5_addr":"127.0.0.1:1080","exits":["exit-alpha","exit-beta"]}"#,
        );
    }

    #[test]
    fn died_carries_a_typed_cause() {
        pin(
            &MixnetStatus {
                mode: Indicator::Died,
                socks5_addr: None,
                exits: Vec::new(),
                bootstrap_detail: None,
                death: Some(DeathReport {
                    at: fixed_at(),
                    detail: Some(NetOpFailure {
                        stage: NetOpStage::TimedOut { after_ms: 25000 },
                        target: "https://indexer.example:443".into(),
                        cause_chain: vec!["deadline elapsed".into()],
                    }),
                }),
            },
            r#"{"mode":"died","death":{"at":1753900000000,"detail":{"stage":{"timed-out":{"after_ms":25000}},"target":"https://indexer.example:443","cause_chain":["deadline elapsed"]}}}"#,
        );
    }

    #[test]
    fn died_without_a_held_cause_omits_the_detail() {
        pin(
            &MixnetStatus {
                mode: Indicator::Died,
                socks5_addr: None,
                exits: Vec::new(),
                bootstrap_detail: None,
                death: Some(DeathReport {
                    at: fixed_at(),
                    detail: None,
                }),
            },
            r#"{"mode":"died","death":{"at":1753900000000}}"#,
        );
    }

    #[test]
    fn a_hostile_at_lifts_to_a_result_not_a_panic() {
        // u64::MAX milliseconds since the epoch overflows Windows'
        // SystemTime representation while Linux absorbs it, so the codec's
        // checked add may legitimately return either verdict by platform.
        // The contract under test is only that hostile wire always reaches
        // a Result, never a panic — the failure an unchecked
        // `UNIX_EPOCH + Duration` exhibited on Windows.
        let hostile = format!(r#"{{"mode":"died","death":{{"at":{}}}}}"#, u64::MAX);
        let _ = serde_json::from_str::<MixnetStatus>(&hostile);
    }

    #[test]
    fn stray_evidence_is_refused() {
        // Each field lifts only inside its one mode; anywhere else the
        // wire is refused rather than silently carried, keeping the
        // struct's mode-scoping invariant true on both directions.
        for hostile in [
            r#"{"mode":"unattached","socks5_addr":"127.0.0.1:1080"}"#,
            r#"{"mode":"ready","bootstrap_detail":"connecting to gateway"}"#,
            r#"{"mode":"bootstrapping","death":{"at":0}}"#,
            r#"{"mode":"bootstrapping","exits":["exit-alpha"]}"#,
        ] {
            assert!(
                serde_json::from_str::<MixnetStatus>(hostile).is_err(),
                "lifted stray evidence: {hostile}"
            );
        }
    }

    #[test]
    fn every_stage_token_is_its_display_mint() {
        // Display's kebab rendering is the mint; the serde discriminant
        // must never drift from it. The match makes the list exhaustive:
        // a new variant fails to compile here until its token is pinned.
        let unit_stages = [
            NetOpStage::ProxyLaunch,
            NetOpStage::RouteResolution,
            NetOpStage::RemoteConnect,
            NetOpStage::LocalProxyConnect,
            NetOpStage::SocksHandshake,
            NetOpStage::TunnelTransport,
            NetOpStage::RemoteTls,
            NetOpStage::RemoteHttp,
            NetOpStage::PayloadDecode,
        ];
        for stage in &unit_stages {
            match stage {
                NetOpStage::ProxyLaunch
                | NetOpStage::RouteResolution
                | NetOpStage::RemoteConnect
                | NetOpStage::LocalProxyConnect
                | NetOpStage::SocksHandshake
                | NetOpStage::TunnelTransport
                | NetOpStage::RemoteTls
                | NetOpStage::RemoteHttp
                | NetOpStage::PayloadDecode
                | NetOpStage::TimedOut { .. } => {}
            }
            assert_eq!(
                serde_json::to_string(stage).unwrap(),
                format!("\"{stage}\"")
            );
        }
        // The one fielded variant: the discriminant is still Display's
        // token; the bound rides as a field, not parenthesized text.
        assert_eq!(
            serde_json::to_string(&NetOpStage::TimedOut { after_ms: 7 }).unwrap(),
            r#"{"timed-out":{"after_ms":7}}"#,
        );
    }

    #[test]
    fn a_unit_stage_is_its_kebab_token_not_a_substring() {
        // The discriminant is the same kebab token Display keeps, so a consumer
        // matches the variant; the cause chain stays layered, never joined.
        let failure = NetOpFailure {
            stage: NetOpStage::RemoteTls,
            target: "https://indexer.example:443".into(),
            cause_chain: vec!["handshake failed".into(), "certificate expired".into()],
        };
        assert_eq!(
            serde_json::to_string(&failure).unwrap(),
            r#"{"stage":"remote-tls","target":"https://indexer.example:443","cause_chain":["handshake failed","certificate expired"]}"#,
        );
    }
}

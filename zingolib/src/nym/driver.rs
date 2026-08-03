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

use crate::nym::{DeathReport, MixnetMode};

/// One observation of Mixnet Mode for subscribers: the mode plus the
/// evidence scoped to it. Every field beyond the mode is `None` outside
/// the one mode it belongs to, exactly as the pull accessors are gated.
///
/// This is the wire snapshot the neon boundary carries to zingo-pc, the
/// boundary-carrier consumer ADR 0024 sequences last. The serde form omits
/// every absent field, so the `mode` token alone discriminates and each state
/// carries only its own evidence. The wire is pinned by the `wire_contract`
/// golden test below, the same way [`MixnetMode`]'s five tokens are pinned in
/// `mode.rs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MixnetStatus {
    /// The Mixnet Mode at this observation.
    pub mode: MixnetMode,
    /// The local SOCKS5 address, present exactly while ready.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub socks5_addr: Option<String>,
    /// The transport's latest bootstrap progress line, present only while
    /// bootstrapping, so a subscriber can narrate the connect race.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bootstrap_detail: Option<String>,
    /// The latched death read whole, present only while died.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub death: Option<DeathReport>,
}

impl MixnetStatus {
    /// The snapshot of a slot state that carries no transport — unattached
    /// or switched off — where every transport-scoped field is absent.
    pub(crate) fn slot_only(mode: MixnetMode) -> Self {
        MixnetStatus {
            mode,
            socks5_addr: None,
            bootstrap_detail: None,
            death: None,
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
        tokio::sync::watch::channel(MixnetStatus::slot_only(MixnetMode::Unattached));
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
/// 0024): a desktop session spawns the bundled binary, a platform that
/// hosts the proxy itself hands the wallet an address to attach to.
#[derive(Debug)]
pub enum ProvisionStrategy<'a> {
    /// Spawn the bundled `nym-proxy` binary, resolving its path from the
    /// consumer's platform hints through the one precedence rule in
    /// [`crate::nym::provision`].
    Spawn(crate::nym::provision::SpawnHints<'a>),
    /// Attach to an already-running, platform-hosted SOCKS5 endpoint.
    Attach {
        /// The endpoint's socket address.
        socks5_addr: &'a str,
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
    use crate::nym::{DeathReport, MixnetMode};

    // A fixed latch moment (2025-07-30T18:26:40Z) so the death pins are
    // deterministic; the wire carries it as milliseconds since the epoch.
    fn fixed_at() -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_753_900_000_000)
    }

    fn pin(status: &MixnetStatus, json: &str) {
        assert_eq!(serde_json::to_string(status).unwrap(), json);
        assert_eq!(&serde_json::from_str::<MixnetStatus>(json).unwrap(), status);
    }

    #[test]
    fn unattached_carries_only_its_token() {
        pin(
            &MixnetStatus::slot_only(MixnetMode::Unattached),
            r#"{"mode":"unattached"}"#,
        );
    }

    #[test]
    fn switched_off_carries_only_its_token() {
        pin(
            &MixnetStatus::slot_only(MixnetMode::SwitchedOff),
            r#"{"mode":"switched_off"}"#,
        );
    }

    #[test]
    fn bootstrapping_carries_its_narration() {
        pin(
            &MixnetStatus {
                mode: MixnetMode::Bootstrapping,
                socks5_addr: None,
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
                mode: MixnetMode::Ready,
                socks5_addr: Some("127.0.0.1:1080".into()),
                bootstrap_detail: None,
                death: None,
            },
            r#"{"mode":"ready","socks5_addr":"127.0.0.1:1080"}"#,
        );
    }

    #[test]
    fn died_carries_a_typed_cause() {
        pin(
            &MixnetStatus {
                mode: MixnetMode::Died,
                socks5_addr: None,
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
                mode: MixnetMode::Died,
                socks5_addr: None,
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
        // SystemTime representation, so there the codec's unchecked
        // `UNIX_EPOCH + Duration` panics inside serde; Linux absorbs the
        // same value. The contract under test is that hostile wire always
        // reaches a Result — which verdict it gets may differ by platform
        // until the codec adopts a checked add or a uniform bound.
        let hostile = format!(r#"{{"mode":"died","death":{{"at":{}}}}}"#, u64::MAX);
        let _ = serde_json::from_str::<MixnetStatus>(&hostile);
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

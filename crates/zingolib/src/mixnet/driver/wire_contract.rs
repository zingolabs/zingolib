use std::time::{Duration, UNIX_EPOCH};

use zl_netutils::diag::{NetOpFailure, NetOpStage};

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

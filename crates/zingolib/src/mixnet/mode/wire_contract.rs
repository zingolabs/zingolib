use std::str::FromStr as _;

use super::Indicator;

/// The ratified tokens, pinned literally so a rename in `as_str` cannot
/// pass silently: this list is the wire contract of ADR 0024.
const RATIFIED_TOKENS: [(Indicator, &str); 6] = [
    (Indicator::Unattached, "unattached"),
    (Indicator::SwitchedOff, "switched_off"),
    (Indicator::Bootstrapping, "bootstrapping"),
    (Indicator::Ready, "ready"),
    (
        Indicator::PreviouslyProvenThisEpoch,
        "previously_proven_this_epoch",
    ),
    (Indicator::Died, "died"),
];

#[test]
fn every_state_mints_its_ratified_token() {
    for (mode, token) in RATIFIED_TOKENS {
        assert_eq!(mode.as_str(), token);
        assert_eq!(mode.to_string(), token);
    }
}

#[test]
fn serde_and_as_str_agree_on_every_state() {
    for mode in Indicator::ALL {
        let json = serde_json::to_string(&mode).expect("serialization is infallible");
        assert_eq!(json, format!("{:?}", mode.as_str()));
        let back: Indicator = serde_json::from_str(&json).expect("the minted token parses back");
        assert_eq!(back, mode);
    }
}

#[test]
fn from_str_round_trips_every_state() {
    for mode in Indicator::ALL {
        assert_eq!(Indicator::from_str(mode.as_str()), Ok(mode));
    }
}

#[test]
fn the_retired_off_token_is_rejected() {
    // "off" is the conflation the five-state decomposition retired; a
    // parser that accepts it would quietly reunify consent with absence.
    assert!(Indicator::from_str("off").is_err());
    assert!(serde_json::from_str::<Indicator>("\"off\"").is_err());
}

/// HYPOTHESIS: a status in stale-proven mode carries its address and
/// exits across the wire whole, because the mode routes exactly as
/// Ready. Falsified if the guard rejects the evidence.
#[test]
fn a_stale_proven_status_round_trips_with_its_evidence() {
    let status = crate::mixnet::MixnetStatus {
        mode: Indicator::PreviouslyProvenThisEpoch,
        socks5_addr: Some("127.0.0.1:1080".parse().expect("the test address parses")),
        exits: vec![crate::mixnet::ExitNodeId::from("exit-alpha")],
        bootstrap_detail: None,
        death: None,
    };
    let json = serde_json::to_string(&status).expect("serialization is infallible");
    let back: crate::mixnet::MixnetStatus =
        serde_json::from_str(&json).expect("a published status parses back");
    assert_eq!(back.mode, Indicator::PreviouslyProvenThisEpoch);
    assert_eq!(back.socks5_addr, status.socks5_addr);
    assert_eq!(back.exits, status.exits);
}

/// HYPOTHESIS: whatever raw fields a slot holds, the evidenced
/// constructor emits a status every mode round-trips, so the publisher
/// and the wire guard agree by construction. Falsified if any mode's
/// published shape fails to deserialize.
#[test]
fn every_published_shape_round_trips() {
    for mode in Indicator::ALL {
        let published = crate::mixnet::MixnetStatus::evidenced(
            mode,
            Some("127.0.0.1:1080".parse().expect("the test address parses")),
            vec![crate::mixnet::ExitNodeId::from("exit-alpha")],
            Some(crate::mixnet::DeathReport {
                at: std::time::SystemTime::UNIX_EPOCH,
                detail: None,
            }),
        );
        let json = serde_json::to_string(&published).expect("serialization is infallible");
        let back: Result<crate::mixnet::MixnetStatus, _> = serde_json::from_str(&json);
        assert!(
            back.is_ok(),
            "mode {} publishes a shape its own wire refuses: {json}",
            mode.as_str()
        );
    }
}

#[test]
fn all_is_exhaustive() {
    // A new variant must join ALL: this match goes non-exhaustive the
    // moment one is added, and ALL's length is pinned by its type.
    for mode in Indicator::ALL {
        match mode {
            Indicator::Unattached
            | Indicator::SwitchedOff
            | Indicator::Bootstrapping
            | Indicator::Ready
            | Indicator::PreviouslyProvenThisEpoch
            | Indicator::Died => {}
        }
    }
}

#![forbid(unsafe_code)]

//! Round-trip tests over the FFI wire encoding. `Lower` serializes a value to
//! the exact byte form uniffi sends across the boundary and `Lift` is what the
//! receiving side runs, so lower-then-lift proves both directions agree — the
//! coherence the host-language bindings depend on — in safe Rust, hermetically.
//! Live start/stop semantics stay on the device-smoke rung (#2507).

use uniffi::{Lift, Lower};
use zingo_nym_proxy_ffi::{ProxyDeathReason, ProxyFfiError, Socks5Endpoint, UniFfiTag};

/// Send `value` through the wire encoding both directions and hand back what
/// the far side would see.
fn round_trip<T: Lower<UniFfiTag> + Lift<UniFfiTag, FfiType = <T as Lower<UniFfiTag>>::FfiType>>(
    value: T,
) -> T {
    T::try_lift(T::lower(value)).expect("lifting a freshly lowered value must succeed")
}

#[test]
fn socks5_endpoint_survives_the_wire_encoding() {
    let endpoint = Socks5Endpoint {
        host: "127.0.0.1".to_string(),
        port: 43210,
    };
    assert_eq!(round_trip(endpoint.clone()), endpoint);
}

#[test]
fn socks5_endpoint_port_extremes_survive_the_wire_encoding() {
    for port in [0u16, 1, u16::MAX] {
        let endpoint = Socks5Endpoint {
            host: "127.0.0.1".to_string(),
            port,
        };
        assert_eq!(round_trip(endpoint.clone()), endpoint);
    }
}

#[test]
fn death_reason_survives_the_wire_encoding_including_non_ascii_detail() {
    let reason = ProxyDeathReason::MixnetDisconnected {
        detail: "gateway went away — упал — 途絶".to_string(),
    };
    assert_eq!(round_trip(reason.clone()), reason);
}

#[test]
fn every_error_variant_survives_the_wire_encoding() {
    let variants = [
        ProxyFfiError::Runtime {
            reason: "no threads".to_string(),
        },
        ProxyFfiError::Connect {
            reason: "gateway refused".to_string(),
        },
        ProxyFfiError::Address {
            reason: "\"not-an-address\": invalid socket address syntax".to_string(),
        },
    ];
    for error in variants {
        assert_eq!(round_trip(error.clone()), error);
    }
}

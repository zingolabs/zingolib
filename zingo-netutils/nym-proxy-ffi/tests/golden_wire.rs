#![forbid(unsafe_code)]

//! Golden wire-format contract tests: the three-language pins.
//!
//! Every value the nym FFI functions carry across the boundary —
//! [`Socks5Endpoint`] from `socks5_endpoint()`, [`ProxyDeathReason`] into
//! `ProxyDeathObserver::on_death`, and every [`ProxyFfiError`] variant
//! `start()` can raise — has its uniffi wire encoding pinned as a hex file
//! under `test-data/golden/`. This suite asserts both directions against the
//! pins; the Kotlin and Swift suites under `contract-tests/` assert their
//! generated converters against the *same files*, so all three languages
//! enforce one contract at test time.
//!
//! The pins are never-regenerate contract artifacts. A mismatch here means
//! the wire encoding changed — a breaking contract change to every shipped
//! binding — and must be decided deliberately, not blessed away. The ignored
//! `bless_missing_goldens` test only creates pins that do not exist yet; it
//! refuses to overwrite.

use std::{fs, path::PathBuf};

use uniffi::{Lift, Lower};
use zingo_nym_proxy_ffi::{ProxyDeathReason, ProxyFfiError, Socks5Endpoint, UniFfiTag};

/// The canonical endpoint values, shared verbatim with the Kotlin and Swift
/// suites. Change nothing here without changing all three suites and minting
/// new pins as a deliberate contract revision.
fn canonical_endpoints() -> Vec<(&'static str, Socks5Endpoint)> {
    let at = |port| Socks5Endpoint {
        host: "127.0.0.1".to_string(),
        port,
    };
    vec![
        ("socks5_endpoint", at(43210)),
        ("socks5_endpoint_port_min", at(0)),
        ("socks5_endpoint_port_max", at(u16::MAX)),
    ]
}

/// The canonical death reason; the non-ASCII detail pins UTF-8 handling.
fn canonical_death_reasons() -> Vec<(&'static str, ProxyDeathReason)> {
    vec![(
        "proxy_death_reason_mixnet_disconnected",
        ProxyDeathReason::MixnetDisconnected {
            detail: "gateway went away — упал — 途絶".to_string(),
        },
    )]
}

/// One canonical value per error variant `start()` can raise.
fn canonical_errors() -> Vec<(&'static str, ProxyFfiError)> {
    vec![
        (
            "proxy_ffi_error_runtime",
            ProxyFfiError::Runtime {
                reason: "no threads".to_string(),
            },
        ),
        (
            "proxy_ffi_error_connect",
            ProxyFfiError::Connect {
                reason: "gateway refused".to_string(),
            },
        ),
        (
            "proxy_ffi_error_address",
            ProxyFfiError::Address {
                reason: "\"not-an-address\": invalid socket address syntax".to_string(),
            },
        ),
    ]
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-data/golden")
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    assert!(hex.len().is_multiple_of(2), "golden hex has odd length");
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("golden must be pure hex"))
        .collect()
}

/// The bytes uniffi sends across the boundary for `value`.
fn wire_bytes<T: Lower<UniFfiTag>>(value: T) -> Vec<u8> {
    let mut buf = Vec::new();
    T::write(value, &mut buf);
    buf
}

/// Assert both directions of the contract for one pinned value: lowering
/// produces exactly the golden bytes, and lifting the golden bytes produces
/// exactly the value, consuming the whole pin.
fn assert_pins<T>(name: &str, value: T)
where
    T: Lower<UniFfiTag> + Lift<UniFfiTag> + Clone + PartialEq + std::fmt::Debug,
{
    let path = golden_dir().join(format!("{name}.hex"));
    let golden_hex = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("missing pin {path:?}; run the ignored bless_missing_goldens test once")
    });
    assert_eq!(
        to_hex(&wire_bytes(value.clone())),
        golden_hex.trim(),
        "{name}: lowering drifted from the pinned wire bytes"
    );
    let golden_bytes = from_hex(&golden_hex);
    let mut reader: &[u8] = &golden_bytes;
    let lifted = T::try_read(&mut reader)
        .unwrap_or_else(|e| panic!("{name}: the pinned bytes no longer lift: {e}"));
    assert!(
        reader.is_empty(),
        "{name}: lifting left trailing bytes unread"
    );
    assert_eq!(lifted, value, "{name}: lifting the pin changed the value");
}

#[test]
fn socks5_endpoint_wire_encoding_matches_the_pins() {
    for (name, endpoint) in canonical_endpoints() {
        assert_pins(name, endpoint);
    }
}

#[test]
fn death_reason_wire_encoding_matches_the_pin() {
    for (name, reason) in canonical_death_reasons() {
        assert_pins(name, reason);
    }
}

#[test]
fn every_error_variant_wire_encoding_matches_its_pin() {
    for (name, error) in canonical_errors() {
        assert_pins(name, error);
    }
}

/// Creates any pin that does not exist yet; never overwrites. Re-pinning an
/// existing golden is a deliberate contract revision: delete the file in the
/// same change that justifies the new encoding, then run this.
#[test]
#[ignore = "bless: creates absent golden pins only"]
fn bless_missing_goldens() {
    let dir = golden_dir();
    fs::create_dir_all(&dir).expect("create golden dir");
    let bless = |name: &str, bytes: Vec<u8>| {
        let path = dir.join(format!("{name}.hex"));
        if path.exists() {
            return;
        }
        fs::write(&path, format!("{}\n", to_hex(&bytes))).expect("write pin");
    };
    for (name, endpoint) in canonical_endpoints() {
        bless(name, wire_bytes(endpoint));
    }
    for (name, reason) in canonical_death_reasons() {
        bless(name, wire_bytes(reason));
    }
    for (name, error) in canonical_errors() {
        bless(name, wire_bytes(error));
    }
}

use super::*;

fn scratch_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("a scratch directory")
}

/// The ratified tokens, pinned literally so a rename cannot pass
/// silently: this pair is the on-disk contract of ADR 0025.
#[test]
fn the_minted_tokens_are_pinned() {
    assert_eq!(CONNECTIVITY_CONSENT_FILE, "connectivity-consent");
    assert_eq!(STANDING_ONLINE_TOKEN, "standing-online");
}

/// HYPOTHESIS: an empty data directory holds no consent — first boot
/// reads Unrecorded. Falsified if absence grants anything.
#[test]
fn an_absent_record_is_unrecorded() {
    let dir = scratch_dir();
    assert_eq!(
        load_connectivity_consent(dir.path()),
        ConnectivityConsent::Unrecorded
    );
}

/// HYPOTHESIS: storing round-trips — a stored choice reads back as
/// standing online, with the token written exactly as minted.
#[test]
fn store_and_load_round_trip() {
    let dir = scratch_dir();
    store_standing_online(dir.path()).expect("the store writes");
    assert_eq!(
        load_connectivity_consent(dir.path()),
        ConnectivityConsent::StandingOnline
    );
    let written = std::fs::read_to_string(dir.path().join(CONNECTIVITY_CONSENT_FILE))
        .expect("the record exists");
    assert_eq!(written.trim(), STANDING_ONLINE_TOKEN);
}

/// HYPOTHESIS (fail-closed): an unrecognized record withholds consent —
/// corruption can keep a session offline, never take one online.
/// Falsified if any non-minted content reads as standing online.
#[test]
fn a_corrupt_record_is_unrecorded() {
    let dir = scratch_dir();
    for corrupt in [
        "",
        "online",
        "STANDING-ONLINE",
        "standing-online extra",
        "yes",
    ] {
        std::fs::write(dir.path().join(CONNECTIVITY_CONSENT_FILE), corrupt)
            .expect("the corrupt record writes");
        assert_eq!(
            load_connectivity_consent(dir.path()),
            ConnectivityConsent::Unrecorded,
            "content {corrupt:?} must not grant consent"
        );
    }
}

/// HYPOTHESIS: forgetting removes the record and is idempotent — the
/// outcome (no standing consent) is the contract, not the transition.
#[test]
fn forget_returns_the_store_to_unrecorded_and_is_idempotent() {
    let dir = scratch_dir();
    store_standing_online(dir.path()).expect("the store writes");
    forget_connectivity_consent(dir.path()).expect("the forget removes");
    assert_eq!(
        load_connectivity_consent(dir.path()),
        ConnectivityConsent::Unrecorded
    );
    forget_connectivity_consent(dir.path()).expect("forgetting nothing succeeds");
}

/// HYPOTHESIS: storing into a directory that does not exist yet creates
/// it — the first consent act may precede any wallet file.
#[test]
fn store_creates_the_data_directory() {
    let dir = scratch_dir();
    let nested = dir.path().join("wallets");
    store_standing_online(&nested).expect("the store creates the directory");
    assert_eq!(
        load_connectivity_consent(&nested),
        ConnectivityConsent::StandingOnline
    );
}

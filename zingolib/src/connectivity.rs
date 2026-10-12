//! Connectivity Consent: the stored, wallet-level record of the user's
//! standing choice to go online (ADR 0025).
//!
//! First boot is offline for every consumer. Going online happens only by
//! an explicit act, and the user may store that choice so later sessions
//! attach to the network automatically. This module owns the stored choice
//! and its predicate — consumers render the prompt and pass the acts in,
//! but never re-derive the rules (ADR 0024's session-policy ownership).
//!
//! The store is deliberately fail-closed: an absent, unreadable, or
//! unrecognized record reads as [`ConnectivityConsent::Unrecorded`], which
//! withholds the connection. A corrupted file can therefore only ever keep
//! a session offline, never take one online.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// The file recording the standing choice, kept beside the wallet in the
/// consumer's data directory.
///
/// This name is a MINTED TOKEN and this constant is its sole production
/// site: every consumer reads and writes the record through this module's
/// functions, and none restates the file name in its own code. The
/// enforcement is documentary, as with `NYM_PROXY_ENV` — no test can catch
/// a restated name in an out-of-repo consumer, and importing the module is
/// the whole fix.
pub const CONNECTIVITY_CONSENT_FILE: &str = "connectivity-consent";

/// The file content granting standing online consent. A MINTED TOKEN with
/// the same monopoly as [`CONNECTIVITY_CONSENT_FILE`]; the `wire_contract`
/// tests pin it literally. No other content grants anything.
const STANDING_ONLINE_TOKEN: &str = "standing-online";

/// The user's standing connectivity choice, as read from disk. The outer
/// tier of the wallet's two consent tiers: persistable, in deliberate
/// contrast to the per-session transport consent (Mixnet Mode's switched
/// off), which never is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectivityConsent {
    /// No standing choice is recorded. The ground state: the session starts
    /// offline, and going online requires an explicit act this session.
    /// Equally the reading of an unreadable or unrecognized record.
    Unrecorded,
    /// The user stored the go-online choice: sessions attach to the network
    /// automatically until the record is forgotten.
    StandingOnline,
}

/// The consent file's path inside `data_dir`.
fn consent_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONNECTIVITY_CONSENT_FILE)
}

/// Read the standing choice from `data_dir`. Fail-closed: an absent,
/// unreadable, or unrecognized record is [`ConnectivityConsent::Unrecorded`]
/// — corruption can keep a session offline, never take one online.
pub fn load_connectivity_consent(data_dir: &Path) -> ConnectivityConsent {
    match std::fs::read_to_string(consent_path(data_dir)) {
        Ok(content) if content.trim() == STANDING_ONLINE_TOKEN => {
            ConnectivityConsent::StandingOnline
        }
        Ok(_) | Err(_) => ConnectivityConsent::Unrecorded,
    }
}

/// Record the user's standing online consent in `data_dir`, creating the
/// directory if needed. From then on [`load_connectivity_consent`] answers
/// [`ConnectivityConsent::StandingOnline`] until [`forget_connectivity_consent`]
/// removes the record.
pub fn store_standing_online(data_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(consent_path(data_dir), format!("{STANDING_ONLINE_TOKEN}\n"))
}

/// Remove the standing choice from `data_dir`, returning the store to
/// [`ConnectivityConsent::Unrecorded`]. Removing an absent record succeeds:
/// the outcome, not the transition, is the contract.
pub fn forget_connectivity_consent(data_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(consent_path(data_dir)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        Ok(()) | Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod wire_contract {
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
}

use super::*;
use zcash_protocol::TxId;
use zingolib::lightclient::error::{MigrationError, TransmissionError};
use zingolib::wallet::error::WalletError;

/// (input, is-the-pinned-variant, the pinned variant's name).
type FunnelCase = (LightClientError, fn(&ZingolibError) -> bool, &'static str);

#[test]
fn the_funnel_maps_each_constructible_input_to_its_pinned_variant() {
    let cases: Vec<FunnelCase> = vec![
        (
            LightClientError::SyncLaunchError,
            |e| matches!(e, ZingolibError::Sync(_)),
            "Sync",
        ),
        (
            LightClientError::SyncNotRunning,
            |e| matches!(e, ZingolibError::Sync(_)),
            "Sync",
        ),
        (
            LightClientError::SendError(SendError::NoStoredProposal),
            |e| matches!(e, ZingolibError::Send(_)),
            "Send",
        ),
        (
            LightClientError::SendError(SendError::TransmissionError(
                TransmissionError::TransmissionFailed("node said no".to_string()),
            )),
            |e| matches!(e, ZingolibError::Send(_)),
            "Send",
        ),
        (
            LightClientError::FileError(std::io::Error::other("disk full")),
            |e| matches!(e, ZingolibError::Save(_)),
            "Save",
        ),
        (
            LightClientError::WalletError(WalletError::MnemonicNotFound),
            |e| matches!(e, ZingolibError::Wallet(_)),
            "Wallet",
        ),
        (
            LightClientError::Offline,
            |e| matches!(e, ZingolibError::Offline),
            "Offline",
        ),
        (
            LightClientError::MigrationError(MigrationError::NoMigration),
            |e| matches!(e, ZingolibError::MigrationNotInProgress),
            "MigrationNotInProgress",
        ),
        (
            LightClientError::MigrationError(MigrationError::AlreadyInProgress),
            |e| matches!(e, ZingolibError::MigrationAlreadyInProgress),
            "MigrationAlreadyInProgress",
        ),
        (
            LightClientError::MigrationError(MigrationError::ConsentStale),
            |e| matches!(e, ZingolibError::MigrationConsentStale(_)),
            "MigrationConsentStale",
        ),
        (
            LightClientError::MigrationError(MigrationError::CadenceFixed),
            |e| matches!(e, ZingolibError::MigrationCadenceFixed(_)),
            "MigrationCadenceFixed",
        ),
        (
            LightClientError::MigrationError(MigrationError::PreSignedUnavailable),
            |e| matches!(e, ZingolibError::Migration(_)),
            "Migration",
        ),
        (
            LightClientError::MigrationError(MigrationError::SplitDidNotConverge(3)),
            |e| matches!(e, ZingolibError::MigrationSplit(_)),
            "MigrationSplit",
        ),
        (
            LightClientError::MigrationError(MigrationError::SplitTransactionFailed(
                TxId::from_bytes([0u8; 32]),
            )),
            |e| matches!(e, ZingolibError::MigrationSplit(_)),
            "MigrationSplit",
        ),
        (
            LightClientError::MigrationError(MigrationError::SplitConfirmationTimeout),
            |e| matches!(e, ZingolibError::MigrationSplit(_)),
            "MigrationSplit",
        ),
    ];
    for (input, is_expected, expected_name) in cases {
        let input_text = input.to_string();
        let mapped = ffi_error(input);
        assert!(
            is_expected(&mapped),
            "{input_text:?} must map to the {expected_name} variant, got: {mapped}"
        );
    }
}

#[test]
fn the_funnel_preserves_the_upstream_message_verbatim_in_the_variant() {
    // The bridges forward the Display text as the rejection message, so
    // the funnel must not paraphrase or truncate the upstream prose.
    let upstream = LightClientError::SendError(SendError::TransmissionError(
        TransmissionError::TransmissionFailed("node said no".to_string()),
    ));
    let upstream_text = upstream.to_string();
    let mapped = ffi_error(upstream);
    assert!(
        mapped.to_string().contains(&upstream_text),
        "the mapped error must carry the upstream text \"{upstream_text}\": {mapped}"
    );
}

use super::*;

#[test]
fn a_save_failure_keeps_the_cause_its_wrapper_does_not_print() {
    let wrapped = std::io::Error::other(LightClientError::WalletError(
        zingolib::wallet::error::WalletError::MnemonicNotFound,
    ));
    let text = map_wallet_save(Err(wrapped))
        .expect_err("a failed save is an error")
        .to_string();
    assert!(
        text.contains("Mnemonic not found"),
        "the cause must survive the crossing: {text}"
    );
}

#[test]
fn a_wallet_read_for_another_chain_opens_as_a_chain_mismatch() {
    let mismatch = LightClientError::FileError(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        zingolib::wallet::disk::ChainMismatch {
            saved: "mainnet".to_string(),
            expected: "testnet".to_string(),
        },
    ));
    let error = open_error(&mismatch);
    assert!(
        matches!(&error, ZingolibError::WalletChainMismatch(text)
            if text.contains("wallet chain name mainnet doesn't match expected testnet")),
        "{error:?}"
    );

    let other = LightClientError::FileError(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "invalid account id",
    ));
    assert!(matches!(open_error(&other), ZingolibError::Init(_)));
}

#[test]
fn read_wallet_chain_names_the_chain_of_the_header() {
    let mut bytes = 41u64.to_le_bytes().to_vec();
    bytes.push(0);
    assert_eq!(read_wallet_chain(bytes).unwrap(), "main");

    let mut bytes = 41u64.to_le_bytes().to_vec();
    bytes.push(1);
    assert_eq!(read_wallet_chain(bytes).unwrap(), "test");

    assert!(matches!(
        read_wallet_chain(vec![1, 2, 3]),
        Err(ZingolibError::Read(_))
    ));
}

#[test]
fn a_seed_that_fails_its_checksum_names_the_mnemonic_error_at_init() {
    let bad_checksum = vec!["abandon"; 24].join(" ");
    let error = init_from_seed(bad_checksum, 1, test_connection("", "main", "Low"))
        .expect_err("a seed with a bad checksum does not build a wallet");
    let text = error.to_string();
    // `LightClientError::WalletError` prints only "Wallet error."; the
    // mnemonic cause under it is what the user has to read.
    assert!(
        text.contains("Mnemonic error"),
        "the cause must survive the crossing: {text}"
    );
}

#[test]
fn a_wrapper_that_prints_nothing_of_its_cause_gains_the_whole_chain() {
    let mapped = ffi_error(LightClientError::SyncError(
        pepper_sync::error::SyncError::ChainError(1, 2, 3),
    ));
    let text = mapped.to_string();
    // `LightClientError::SyncError` is declared `#[error("Sync error.")]`,
    // with no interpolation: before the chain walk this was the entire
    // message that crossed.
    assert!(
        text.contains("wallet height 1 is more than 2 blocks ahead"),
        "the cause must survive the crossing: {text}"
    );
    assert!(
        !text.contains(".:"),
        "a layer's sentence period must not collide with the separator: {text}"
    );
}

#[test]
fn a_transparent_wrapper_does_not_repeat_its_cause() {
    let mapped = ffi_error(LightClientError::MixnetNotReady(
        zingolib::mixnet::MixnetNotReady::Bootstrapping,
    ));
    let text = mapped.to_string();
    assert_eq!(
        text.matches("bootstrapping").count(),
        1,
        "`#[error(transparent)]` already prints its source: {text}"
    );
}

#[test]
fn an_interpolating_wrapper_does_not_repeat_its_cause() {
    #[derive(Debug, thiserror::Error)]
    #[error("the leaf failed")]
    struct Leaf;

    #[derive(Debug, thiserror::Error)]
    #[error("wallet error. {0}")]
    struct Interpolating(#[from] Leaf);

    #[derive(Debug, thiserror::Error)]
    #[error("outer")]
    struct Outer(#[from] Interpolating);

    let text = chain_text(&Outer(Interpolating(Leaf)));
    assert_eq!(
        text, "outer: wallet error. the leaf failed",
        "a variant that interpolates its source has already printed it"
    );
}

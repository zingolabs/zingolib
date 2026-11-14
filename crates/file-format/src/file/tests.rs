use crate::{RawMigration, WalletFile};

/// A file whose tail reads cleanly under *both* layouts is genuinely
/// ambiguous, and the reader must refuse it rather than silently substitute
/// one reading's price list and migration state for the other's.
///
/// The decision is exercised directly: constructing a byte string that
/// satisfies both parses is not possible for a migration-free tail (see the
/// parity argument on `resolve_v42_tail`) and impractical otherwise, so the
/// test supplies the four parse outcomes the caller can hand it.
#[test]
fn ambiguous_version_42_tail_is_refused() {
    use zl_common::price::PriceList;

    let parsed = || Ok((PriceList::new(), None));
    let failed = || {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "tail did not parse",
        ))
    };

    let ambiguous = WalletFile::<RawMigration>::resolve_v42_tail(parsed(), Some(parsed()));
    assert_eq!(
        ambiguous
            .expect_err("resolve_v42_tail refuses a tail that parses both ways")
            .kind(),
        std::io::ErrorKind::InvalidData,
        "a tail parsing both ways must be refused, not guessed"
    );

    assert!(
        WalletFile::<RawMigration>::resolve_v42_tail(parsed(), Some(failed())).is_ok(),
        "the canonical layout must win when only it parses"
    );
    assert!(
        WalletFile::<RawMigration>::resolve_v42_tail(parsed(), None).is_ok(),
        "a leading byte that is no bool rules out the pre-release layout"
    );
    assert!(
        WalletFile::<RawMigration>::resolve_v42_tail(failed(), Some(parsed())).is_ok(),
        "the pre-release layout must be accepted when only it parses"
    );
    assert!(
        WalletFile::<RawMigration>::resolve_v42_tail(failed(), Some(failed())).is_err(),
        "a tail parsing neither way must fail"
    );
}

#[cfg(not(feature = "legacy"))]
#[test]
fn a_legacy_file_without_the_legacy_feature_names_the_feature() {
    use zl_common::chain::ChainType;

    let legacy_version: u64 = 31;
    let bytes = legacy_version.to_le_bytes();
    let errors = [
        super::read_chain(&bytes)
            .expect_err("read_chain reads no file below version 32 without the legacy feature"),
        WalletFile::<RawMigration>::read(&bytes[..], ChainType::Mainnet).expect_err(
            "WalletFile::read reads no file below version 32 without the legacy feature",
        ),
    ];
    for error in errors {
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("`legacy` feature"),
            "the error must name the missing feature: {error}"
        );
    }
}

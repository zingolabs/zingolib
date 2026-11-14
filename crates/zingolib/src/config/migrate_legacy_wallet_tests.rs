use super::migrate_legacy_wallet;

struct Dirs {
    _root: tempfile::TempDir,
    legacy: std::path::PathBuf,
    current: std::path::PathBuf,
}

fn dirs() -> Dirs {
    let root = tempfile::tempdir().expect("a temp dir");
    let legacy = root.path().join("home/.zcash");
    let current = root.path().join("share/.zcash");
    std::fs::create_dir_all(&legacy).expect("legacy dir");
    std::fs::create_dir_all(&current).expect("current dir");
    Dirs {
        _root: root,
        legacy,
        current,
    }
}

/// Tests that a wallet only in the legacy directory is copied over, byte for byte, when built.
#[test]
fn copies_a_wallet_left_in_the_legacy_directory() {
    let d = dirs();
    std::fs::write(d.legacy.join("zingo-wallet.dat"), b"the wallet").unwrap();

    let from = migrate_legacy_wallet(&d.legacy, &d.current, "zingo-wallet.dat").unwrap();

    assert_eq!(from, Some(d.legacy.join("zingo-wallet.dat")));
    assert_eq!(
        std::fs::read(d.current.join("zingo-wallet.dat")).unwrap(),
        b"the wallet"
    );
    // The original stays, and no staging file is left behind.
    assert!(d.legacy.join("zingo-wallet.dat").exists());
    assert!(!d.current.join("zingo-wallet.dat.migrating").exists());
}

/// Tests that a wallet already in the current directory is left as it is when the legacy one also holds it.
#[test]
fn leaves_a_wallet_already_in_the_current_directory() {
    let d = dirs();
    std::fs::write(d.legacy.join("zingo-wallet.dat"), b"old").unwrap();
    std::fs::write(d.current.join("zingo-wallet.dat"), b"current").unwrap();

    assert_eq!(
        migrate_legacy_wallet(&d.legacy, &d.current, "zingo-wallet.dat").unwrap(),
        None
    );
    assert_eq!(
        std::fs::read(d.current.join("zingo-wallet.dat")).unwrap(),
        b"current"
    );
}

/// Tests that nothing is copied when the legacy directory does not hold the wallet.
#[test]
fn copies_nothing_for_a_wallet_the_legacy_directory_does_not_hold() {
    let d = dirs();
    std::fs::write(d.legacy.join("wallet.dat"), b"zcashd").unwrap();

    assert_eq!(
        migrate_legacy_wallet(&d.legacy, &d.current, "zingo-wallet.dat").unwrap(),
        None
    );
    assert!(!d.current.join("wallet.dat").exists());
    assert!(!d.current.join("zingo-wallet.dat").exists());
}

/// Tests that a missing legacy directory is not an error.
#[test]
fn copies_nothing_when_there_is_no_legacy_directory() {
    let d = dirs();
    let missing = d.legacy.join("nowhere");

    assert_eq!(
        migrate_legacy_wallet(&missing, &d.current, "zingo-wallet.dat").unwrap(),
        None
    );
}

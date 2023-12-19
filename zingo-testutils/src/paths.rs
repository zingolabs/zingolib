use std::path::PathBuf;

pub fn get_cargo_manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("To be inside a manifested space."))
}

pub fn get_regtest_dir() -> PathBuf {
    get_cargo_manifest_dir().join("regtest")
}

pub fn get_bin_dir() -> PathBuf {
    let mut dir = get_cargo_manifest_dir();
    dir.pop();
    dir.join("zingo-testutils").join("test_binaries").join("bins")
}

use std::process::Command;

include!("src/paths.rs");

fn main() {
    let bin_path = get_cargo_manifest_dir();
    let _output = Command::new("git")
        .current_dir(bin_path)
        .args(["clone", "https://github.com/zingolabs/test_binaries.git"])
        .output()
        .expect("Failed to git clone test binaries.");
}

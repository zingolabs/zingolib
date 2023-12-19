use std::{env, path::PathBuf, process::Command};

// macro_rules! p {
//     ($($tokens: tt)*) => {
//         println!("cargo:warning={}", format!($($tokens)*))
//     }
// }

fn main() {
    let out_dir = env::var_os("OUT_DIR").unwrap();
    let mut bin_path = PathBuf::from(out_dir);
    bin_path.pop();
    bin_path.pop();
    bin_path.pop();
    // p!("{:?}", bin_path);
    let _output = Command::new("git")
        .current_dir(bin_path)
        .args(["clone", "https://github.com/zingolabs/test_binaries.git"])
        .output()
        .expect("Failed to git clone test binaries.");
}

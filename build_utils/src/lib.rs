use std::io::Write;
use std::{env, fs::File, path::Path, process::Command};

pub fn git_description() {
    let _fetch = Command::new("git")
        .args(["fetch", "--tags", "https://github.com/zingolabs/zingolib"])
        .output()
        .expect("Failed to execute git command");
    let output = Command::new("git")
        .args(["describe", "--dirty", "--always", "--long"])
        .output()
        .expect("Failed to execute git command");

    eprintln!("Git command output: {:?}", output);
    println!("Git command output: {:?}", output);

    let git_description = String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_string();

    // Write the git description to a file which will be included in the crate
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("git_description.rs");
    let mut f = File::create(dest_path).unwrap();
    writeln!(
        f,
        "/// The result of running 'git describe' at compile time:\n\
        /// The most recent tag name, the number\n\
        /// of commits above it, and the hash of\n\
        /// the most recent commit\n\
        pub fn git_description() -> &'static str {{\"{}\"}}",
        git_description
    )
    .unwrap();
}

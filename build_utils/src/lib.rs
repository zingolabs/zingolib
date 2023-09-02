use std::io::Write;
use std::{env, fs::File, path::Path, process::Command};

pub fn git_description() {
    let output = Command::new("git")
        .args(["describe", "--dirty"])
        .output()
        .expect("Failed to execute git command");

    eprintln!("Git command output: {:?}", output);

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
        "pub fn git_description() -> &'static str {{\"{}\"}}",
        git_description
    )
    .unwrap();
}

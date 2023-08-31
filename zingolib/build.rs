// build.rs
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;
// use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().build_server(true).compile(
        &["proto/service.proto", "proto/compact_formats.proto"],
        &["proto"],
    )?;
    // println!("cargo:rerun-if-changed=proto/service.proto");
    // // Run `git describe --dirty` to get the description
    // let output = Command::new("git")
    //     .args(["describe", "--dirty"])
    //     .output()
    //     .expect("Failed to execute git command");

    // let git_description = String::from_utf8(output.stdout).unwrap();

    // // Write the git description to a file which will be included in the crate
    // let out_dir = env::var("OUT_DIR").unwrap();
    // let dest_path = Path::new(&out_dir).join("git_description.rs");
    // let mut f = File::create(dest_path).unwrap();
    // writeln!(
    //     f,
    //     "pub fn git_description() -> &'static str {{\"{}\"}}",
    //     git_description
    // )
    // .unwrap();

    let out_dir = env::var_os("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("git_description.rs");
    let mut f = File::create(dest_path).unwrap();
    writeln!(
        f,
        "pub fn git_description() -> &'static str {{\"mob-release-test\"}}"
    )
    .unwrap();
    println!("cargo:rerun-if-changed=proto/service.proto");
    Ok(())
}

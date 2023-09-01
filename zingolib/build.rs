// build.rs
use build_utils;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().build_server(true).compile(
        &["proto/service.proto", "proto/compact_formats.proto"],
        &["proto"],
    )?;
    println!("cargo:rerun-if-changed=proto/service.proto");
    // Run `git describe --dirty` to get the description
    build_utils::git_description();
    Ok(())
}

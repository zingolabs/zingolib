fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().build_server(true).compile(
        &[
            "../zingolib/proto/compact_formats.proto",
            "proto/darkside.proto",
            "../zingolib/proto/service.proto",
        ],
        &["proto", "../zingolib/proto"],
    )?;
    println!("cargo:rerun-if-changed=proto/darkside.proto");
    Ok(())
}

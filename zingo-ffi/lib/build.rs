#![forbid(unsafe_code)]

use std::io::Write;
use std::path::Path;
use std::{env, fs::File};

/// The variable through which the consumer passes its finished `zm_` descriptor.
const DESCRIPTOR_VARIABLE: &str = "ZINGO_MOBILE_DESCRIPTOR";

/// The descriptor of a build that the consumer did not describe.
const UNKNOWN_DESCRIPTOR: &str = "zm_unknown";

/// The prefix that every consumer descriptor carries.
const DESCRIPTOR_PREFIX: &str = "zm_";

// Emitting any directive disables cargo's whole-package fallback, so the
// watch set must cover the uniffi scaffolding inputs (src/) as well as
// the variable behind the zm descriptor.
fn register_rerun_watches() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed={DESCRIPTOR_VARIABLE}");
}

/// The consumer's descriptor when it carries the `zm_` prefix, else the unknown descriptor.
fn descriptor(passed: Option<&str>) -> String {
    passed
        .filter(|value| value.starts_with(DESCRIPTOR_PREFIX))
        .unwrap_or(UNKNOWN_DESCRIPTOR)
        .to_string()
}

// The consumer computes its descriptor from its own checkout, which this
// crate cannot see from the submodule, and passes it through the variable.
fn zm_description() {
    let description = descriptor(env::var(DESCRIPTOR_VARIABLE).ok().as_deref());
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("zm_description.rs");
    let mut f = File::create(dest_path).unwrap();
    writeln!(
        f,
        "/// The zingo-mobile part of the build descriptor, as the consumer\n\
        /// passed it: `zm_<tag#>` on a `zingo-<tag#>` release tag, else\n\
        /// `zm_<ver>_<hash5>`, each with `_dirty` for a modified tree\n\
        pub fn zm_description() -> &'static str {{\"{description}\"}}"
    )
    .unwrap();
}

fn main() {
    register_rerun_watches();
    uniffi_build::generate_scaffolding("src/zingo.udl").expect("A valid UDL file");
    zm_description();
}

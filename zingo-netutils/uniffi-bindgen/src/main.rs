#![forbid(unsafe_code)]

//! Project-local `uniffi-bindgen` (the uniffi-recommended pattern): pinned to
//! the exact `uniffi` version the shim compiles against, so generated bindings
//! can never drift from the scaffolding. Run it in library mode against the
//! built cdylib, e.g.:
//!
//! ```text
//! cargo run --package zingo-uniffi-bindgen -- \
//!     generate --library target/debug/libzingo_nym_proxy_ffi.so \
//!     --language kotlin --out-dir bindings/kotlin
//! ```

fn main() {
    uniffi::uniffi_bindgen_main()
}

#![forbid(unsafe_code)]

//! Project-local `uniffi-bindgen` (the uniffi-recommended pattern) for the Nym
//! proxy shim: pinned to the exact `uniffi` version `zingo-nym-proxy-ffi`
//! compiles against, so generated bindings can never drift from the
//! scaffolding. Run it in library mode against a built shim library — library
//! mode reads the UniFFI metadata statically, so a cross-compiled Android
//! `.so` works on the host:
//!
//! ```text
//! cargo run --package zingo-uniffi-bindgen -- \
//!     generate --library <path>/libzingo_nym_proxy_ffi.so \
//!     --language kotlin --out-dir <out>
//! ```
//!
//! `consume-android-shim` (the workbench crate) drives this against the
//! bundle staged from zingolib's `makers bundle-android-shim`.

fn main() {
    uniffi::uniffi_bindgen_main()
}

#![forbid(unsafe_code)]

//! Project-local `uniffi-bindgen` for the Nym proxy shim, pinned to the
//! `uniffi` version `zingo-nym-proxy-ffi` compiles against. Run it in library
//! mode against a built shim library. Library mode reads the UniFFI metadata
//! statically, so a cross-compiled Android `.so` works on the host:
//!
//! ```text
//! cargo run --package zingo-uniffi-bindgen --bin zingo-uniffi-bindgen -- \
//!     generate --library <path>/libzingo_nym_proxy_ffi.so \
//!     --language kotlin --out-dir <out>
//! ```
//!
//! `scripts/generate_kotlin_bindings.mjs` and `consume-android-shim` (the
//! workbench crate) drive it.

fn main() {
    uniffi_shim::uniffi_bindgen_main()
}

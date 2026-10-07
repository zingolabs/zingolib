#![forbid(unsafe_code)]

//! Project-local `uniffi-bindgen` for the wallet library and the Nym proxy
//! shim, on the workspace `uniffi` pin that both compile against. Run it in
//! library mode against a built library. Library mode reads the UniFFI
//! metadata statically, so a cross-compiled Android `.so` works on the host:
//!
//! ```text
//! cargo run --package zingo-uniffi-bindgen --bin zingo-uniffi-bindgen -- \
//!     generate --library <path>/libzingo.so \
//!     --language kotlin --out-dir <out>
//! ```
//!
//! `scripts/generate_kotlin_bindings.mjs` and `consume-android-shim` (the
//! workbench crate) drive it.

fn main() {
    uniffi::uniffi_bindgen_main()
}

#![forbid(unsafe_code)]

//! Project-local `uniffi-bindgen` for the wallet library, on the workspace
//! `uniffi` pin that `zingo` (zingo-ffi/lib) compiles against. Run it
//! against the UDL, which needs the bindgen binary alone:
//!
//! ```text
//! cargo run --package zingo-uniffi-bindgen --bin zingo-wallet-uniffi-bindgen -- \
//!     generate zingo-ffi/lib/src/zingo.udl --language kotlin --out-dir <out>
//! ```
//!
//! `build-binding-layer` (tools/workbench) drives it.

fn main() {
    uniffi::uniffi_bindgen_main()
}

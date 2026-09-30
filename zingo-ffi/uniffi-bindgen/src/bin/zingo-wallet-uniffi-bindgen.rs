#![forbid(unsafe_code)]

//! Project-local `uniffi-bindgen` for the wallet library, pinned to the
//! workspace `uniffi` version `zingo` (rust/lib) compiles against. Run it
//! against the UDL. It does not compile the wallet:
//!
//! ```text
//! cargo run --package zingo-uniffi-bindgen --bin zingo-wallet-uniffi-bindgen -- \
//!     generate rust/lib/src/zingo.udl --language kotlin --out-dir <out>
//! ```
//!
//! `scripts/generate_kotlin_bindings.mjs` drives it.

fn main() {
    uniffi_wallet::uniffi_bindgen_main()
}

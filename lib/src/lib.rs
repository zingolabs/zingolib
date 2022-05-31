#[macro_use]
extern crate rust_embed;

pub mod blaze;
pub mod commands;
pub mod compact_formats;
pub mod grpc_connector;
pub mod lightclient;
pub mod lightwallet;

#[cfg(feature = "embed_params")]
#[derive(RustEmbed)]
#[folder = "zcash-params/"]
pub struct SaplingParams;

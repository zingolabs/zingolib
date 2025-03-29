//! General library utilities such as parsing and conversions.

pub mod conversion;
pub mod error;

macro_rules! build_method {
    ($name:ident, $localtype:ty) => {
        #[doc = "Set the $name field of the builder."]
        pub fn $name(&mut self, $name: $localtype) -> &mut Self {
            self.$name = Some($name);
            self
        }
    };
}
#[cfg(any(test, feature = "test-elevation"))]
macro_rules! build_method_push {
    ($name:ident, $localtype:ty) => {
        #[doc = "Push a $ty to the builder."]
        pub fn $name(&mut self, $name: $localtype) -> &mut Self {
            self.$name.push($name);
            self
        }
    };
}
#[allow(unused_macros)]
#[cfg(any(test, feature = "test-elevation"))]
macro_rules! build_push_list {
    ($name:ident, $builder:ident, $struct:ident) => {
        for i in &$builder.$name {
            $struct.$name.push(i.build());
        }
    };
}

use std::path::Path;

pub(crate) use build_method;
#[cfg(any(test, feature = "test-elevation"))]
pub(crate) use build_method_push;
#[allow(unused_imports)]
#[cfg(any(test, feature = "test-elevation"))]
pub(crate) use build_push_list;
use tokio::io::AsyncWriteExt as _;
use zcash_primitives::consensus::NetworkConstants;

/// Take a P2PKH taddr and interpret it as a tex addr
#[cfg(feature = "test-elevation")]
pub fn interpret_taddr_as_tex_addr(
    taddr_bytes: [u8; 20],
    p: &impl zcash_primitives::consensus::Parameters,
) -> String {
    bech32::encode::<bech32::Bech32m>(
        bech32::Hrp::parse_unchecked(p.network_type().hrp_tex_address()),
        &taddr_bytes,
    )
    .unwrap()
}

/// Writes `bytes` to file at `path`.
pub(crate) async fn write_to_path(path: &Path, bytes: Vec<u8>) -> std::io::Result<()> {
    let mut file = tokio::fs::File::create(path).await?;
    file.write_all(&bytes).await
}

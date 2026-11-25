//! The chain-reply domain vocabulary, split out from `zl-common` so that a crate speaking only
//! the wire protocol (`zl-netutils`) can name these types without inheriting the wallet stack
//! (`orchard`, `sapling-crypto`, `zcash_keys`, ...) that `zl-common` carries for its own, larger
//! job. `zl-common` re-exports this module at the same path (`zl_common::chain`) for every
//! consumer that already depends on the wallet stack anyway.

pub mod chain;

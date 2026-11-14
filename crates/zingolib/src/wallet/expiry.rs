//! The transaction expiry delta.
//!
//! ZIP 203 sizes the delta in blocks for a wall-clock duration of about 50
//! minutes, and NU7's shorter block spacing rescales it (zingo-adrs
//! zingolib/0056). One rule lives here and every build site asks it, so a
//! proposal's expiry is known before the builder exists; the values are the
//! builder's own per-branch constants.

use zcash_primitives::transaction::builder::{
    BLOSSOM_DEFAULT_TX_EXPIRY_DELTA, NU7_DEFAULT_TX_EXPIRY_DELTA,
};
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};

/// The expiry delta ZIP 203 recommends from NU7 activation: the 50 minutes
/// [`BLOSSOM_DEFAULT_TX_EXPIRY_DELTA`] held at the 75-second spacing, in
/// 25-second blocks.
pub const NU7_TX_EXPIRY_DELTA: u32 = NU7_DEFAULT_TX_EXPIRY_DELTA;

/// The expiry delta of a transaction targeting `target_height`:
/// [`NU7_TX_EXPIRY_DELTA`] from the NU7 activation,
/// [`BLOSSOM_DEFAULT_TX_EXPIRY_DELTA`] below it and on a chain that never
/// activates NU7.
pub fn tx_expiry_delta(chain: &impl Parameters, target_height: BlockHeight) -> u32 {
    let nu7_active = chain
        .activation_height(NetworkUpgrade::Nu7)
        .is_some_and(|activation| target_height >= activation);
    if nu7_active {
        NU7_TX_EXPIRY_DELTA
    } else {
        BLOSSOM_DEFAULT_TX_EXPIRY_DELTA
    }
}

/// The expiry height of a transaction targeting `target_height`.
pub fn tx_expiry_height(chain: &impl Parameters, target_height: BlockHeight) -> BlockHeight {
    target_height + tx_expiry_delta(chain, target_height)
}

#[cfg(test)]
mod tests;

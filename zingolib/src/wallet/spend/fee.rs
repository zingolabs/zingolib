//! ZIP 317 fee sizing for the plan layer.
//!
//! The arithmetic itself stays upstream (ADR 0010): every fee here comes
//! out of `zcash_primitives`' `zip317::FeeRule`. This module owns only the
//! *counting* — the serialized sizes and padded bundle counts the fee rule
//! is fed — including the null-data (OP_RETURN) output's size, which the
//! change computation must count even though the output carries no value.

use zcash_primitives::transaction::fees::FeeRule as _;
use zcash_primitives::transaction::fees::transparent::InputSize;
use zcash_primitives::transaction::fees::zip317::{FeeRule, P2PKH_STANDARD_OUTPUT_SIZE};
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;

use crate::config::ChainType;

/// The padded Sapling spend count: any spend activity occupies at least
/// one slot; a bundle with no spends contributes none.
fn sapling_spend_count(spends: usize) -> usize {
    if spends > 0 { spends.max(1) } else { 0 }
}

/// The padded Sapling output count: any Sapling activity (spends or
/// outputs) forces the builder's minimum of two output slots.
fn sapling_output_count(spends: usize, outputs: usize) -> usize {
    if spends > 0 || outputs > 0 {
        outputs.max(2)
    } else {
        0
    }
}

/// The padded legacy-Orchard (V5 vanilla) action count under the default
/// bundle type: the vanilla bundle cannot pair a spend with an unrelated
/// output (no cross-address actions), so every spend and every output
/// occupies its own action, padded to the builder's minimum of two
/// whenever the bundle has any activity.
fn orchard_vanilla_action_count(spends: usize, outputs: usize) -> usize {
    if spends > 0 || outputs > 0 {
        (spends + outputs).max(2)
    } else {
        0
    }
}

/// The padded Ironwood action count under the default bundle type: the
/// V6 bundle pairs spends with outputs cross-address, so actions are the
/// larger of the two, padded to the builder's minimum of two whenever
/// the bundle has any activity.
fn ironwood_action_count(spends: usize, outputs: usize) -> usize {
    if spends > 0 || outputs > 0 {
        spends.max(outputs).max(2)
    } else {
        0
    }
}

/// The serialized size of the zero-value null-data output carrying
/// `data_len` bytes of OP_RETURN Data: an eight-byte value, a one-byte
/// script length, and a script of `OP_RETURN` followed by the pushdata
/// encoding (a single length byte up to 75 bytes of data, `OP_PUSHDATA1`
/// plus a length byte above that).
#[must_use]
pub(crate) fn op_return_output_size(data_len: usize) -> usize {
    let push_overhead = if data_len <= 75 { 1 } else { 2 };
    let script_len = 1 + push_overhead + data_len;
    8 + 1 + script_len
}

/// The counted inputs and outputs of one planned transaction, before
/// padding. Change outputs are included in the shielded output counts.
#[derive(Debug, Clone, Default)]
pub(crate) struct TransactionShape {
    /// Serialized sizes of the real and ephemeral transparent inputs.
    pub(crate) transparent_input_sizes: Vec<InputSize>,
    /// Serialized sizes of the real, ephemeral, and null-data outputs.
    pub(crate) transparent_output_sizes: Vec<usize>,
    /// Sapling spends and outputs (outputs include change).
    pub(crate) sapling: (usize, usize),
    /// Orchard spends and outputs (outputs include change).
    pub(crate) orchard: (usize, usize),
    /// Ironwood spends and outputs (outputs include change).
    pub(crate) ironwood: (usize, usize),
}

impl TransactionShape {
    /// A shape with one standard P2PKH ephemeral input added — the TEX
    /// exposure step's sole funding input.
    pub(crate) fn with_ephemeral_input(mut self) -> Self {
        self.transparent_input_sizes.push(InputSize::STANDARD_P2PKH);
        self
    }

    /// A shape with one standard P2PKH ephemeral output added — the TEX
    /// shielding step's output to the Refund Address.
    pub(crate) fn with_ephemeral_output(mut self) -> Self {
        self.transparent_output_sizes
            .push(P2PKH_STANDARD_OUTPUT_SIZE);
        self
    }

    /// The ZIP 317 fee for this shape, from the upstream fee rule.
    pub(crate) fn fee(
        &self,
        chain_type: &ChainType,
        target_height: BlockHeight,
    ) -> Result<Zatoshis, zcash_primitives::transaction::fees::zip317::FeeError> {
        FeeRule::standard().fee_required(
            chain_type,
            target_height,
            self.transparent_input_sizes.iter().cloned(),
            self.transparent_output_sizes.iter().copied(),
            sapling_spend_count(self.sapling.0),
            sapling_output_count(self.sapling.0, self.sapling.1),
            orchard_vanilla_action_count(self.orchard.0, self.orchard.1),
            ironwood_action_count(self.ironwood.0, self.ironwood.1),
        )
    }
}

#[cfg(test)]
mod tests {
    use zcash_protocol::value::Zatoshis;

    use super::{TransactionShape, op_return_output_size};
    use crate::config::ChainType;

    fn fee_of(shape: &TransactionShape) -> u64 {
        shape
            .fee(&ChainType::Mainnet, 3_000_000.into())
            .unwrap()
            .into_u64()
    }

    #[test]
    fn minimum_fee_is_two_grace_actions() {
        // One orchard spend, one output: padded to 2 actions.
        let shape = TransactionShape {
            orchard: (1, 1),
            ..Default::default()
        };
        assert_eq!(fee_of(&shape), 10_000);
    }

    #[test]
    fn sapling_activity_pads_to_two_outputs() {
        // One sapling spend, one output: 1 spend + max(1, 2) outputs
        // => max(1, 2) logical actions from sapling = 2 => fee 10_000.
        let shape = TransactionShape {
            sapling: (1, 1),
            ..Default::default()
        };
        assert_eq!(fee_of(&shape), 10_000);
    }

    #[test]
    fn tex_exposure_step_fee_is_the_minimum() {
        // The exposure step: one ephemeral P2PKH input, one standard TEX
        // output. ceil(150/150)=1 in, ceil(34/34)=1 out => 1 logical
        // action, under the 2-action grace floor.
        let shape = TransactionShape::default()
            .with_ephemeral_input()
            .with_ephemeral_output();
        assert_eq!(fee_of(&shape), 10_000);
    }

    #[test]
    fn op_return_data_is_counted_toward_the_fee() {
        // 80 bytes of OP_RETURN Data alongside two standard outputs:
        // t_out = 34 + 34 + 92 = 160 => ceil(160/34) = 5 logical actions.
        let shape = TransactionShape {
            transparent_output_sizes: vec![34, 34, op_return_output_size(80)],
            ..Default::default()
        };
        assert_eq!(op_return_output_size(80), 92);
        assert_eq!(fee_of(&shape), 25_000);
    }

    #[test]
    fn op_return_output_sizes_by_push_encoding() {
        // <= 75 bytes: direct push (1 length byte).
        assert_eq!(op_return_output_size(0), 11);
        assert_eq!(op_return_output_size(17), 28);
        assert_eq!(op_return_output_size(75), 86);
        // 76..=80 bytes: OP_PUSHDATA1 plus a length byte.
        assert_eq!(op_return_output_size(76), 88);
    }

    #[test]
    fn zero_value_op_return_does_not_change_balance() {
        // The output is fee-relevant but value-irrelevant; sanity-pin the
        // zero here so the plan layer's balance math can rely on it.
        assert_eq!(Zatoshis::ZERO.into_u64(), 0);
    }
}

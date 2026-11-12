//! The wallet's effective fee rule: ZIP 317, with the marginal fee raised to
//! whatever the connected indexer's backend reports, capped at the weight
//! ratio over the standard past which extra fee buys no inclusion.

use zcash_primitives::transaction::fees::zip317;
use zcash_protocol::value::Zatoshis;

use zingo_netutils::lightwallet_protocol::StandardFee;

/// The estimator version whose reported marginal fee is the static ZIP 317
/// one, the only semantics the wallet can interpret. Any other version's
/// reports are ignored wholesale.
const STATIC_ZIP317_ESTIMATOR: u32 = 0;

/// The ZIP 317 weight-ratio cap, Revision 1: the block-template lottery
/// reads any fee above this multiple of the conventional fee as exactly
/// this multiple, so a marginal fee beyond it buys no inclusion probability
/// (<https://zips.z.cash/zip-0317#recommended-algorithm-for-block-template-construction>).
/// A report past the cap is honored at the cap: a lie costs at most the
/// cap's overcharge on one transaction, and a genuine demand above the cap
/// is indistinguishable on the network from the cap itself.
const WEIGHT_RATIO_CAP: u64 = 4;

/// A backend's `GetStandardFee` report, cached in wallet state at first
/// contact. Copyable and proto-free, so an event can carry it unchanged
/// (zingolabs/zingolib#2820).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardFeeReport {
    /// The reported marginal fee, zatoshis per logical action.
    pub fee_zat: u64,
    /// The backend fee-estimator version, [`STATIC_ZIP317_ESTIMATOR`] for the
    /// static ZIP 317 marginal fee.
    pub estimator_version: u32,
}

impl From<StandardFee> for StandardFeeReport {
    fn from(fee: StandardFee) -> Self {
        Self {
            fee_zat: fee.fee_zat,
            estimator_version: fee.estimator_version,
        }
    }
}

impl Default for StandardFeeReport {
    fn default() -> Self {
        Self {
            fee_zat: zip317::MARGINAL_FEE.into_u64(),
            estimator_version: STATIC_ZIP317_ESTIMATOR,
        }
    }
}

/// The fee rule every proposal, shield, OP_RETURN send, and migration
/// prices with: the standard ZIP 317 rule whose marginal fee is the node's
/// reported standard fee clamped between the ZIP 317 standard marginal fee
/// and the weight-ratio cap over it. The wallet never pays below the
/// standard, and never pays past the point where extra fee stops buying
/// inclusion. Reports it cannot interpret (an unknown estimator version)
/// or never received (an old backend answers `Unimplemented`, any transport
/// failure) leave the standard rule standing.
pub(crate) fn effective_fee_rule(reported: Option<StandardFeeReport>) -> zip317::FeeRule {
    let standard = zip317::FeeRule::standard();
    let Some(reported) =
        reported.filter(|report| report.estimator_version == STATIC_ZIP317_ESTIMATOR)
    else {
        return standard;
    };
    let standard_marginal = zip317::MARGINAL_FEE.into_u64();
    let marginal_fee = reported
        .fee_zat
        .clamp(standard_marginal, WEIGHT_RATIO_CAP * standard_marginal);
    if marginal_fee == standard_marginal {
        return standard;
    }
    // The clamp bounds the fee at the cap, well inside Zatoshis' range, and
    // `non_standard` refuses only zero P2PKH sizes, which the standard sizes
    // are not. The standard rule is the safe answer in both impossible
    // branches.
    zip317::FeeRule::non_standard(
        Zatoshis::from_u64(marginal_fee).unwrap_or(zip317::MARGINAL_FEE),
        zip317::GRACE_ACTIONS,
        zip317::P2PKH_STANDARD_INPUT_SIZE,
        zip317::P2PKH_STANDARD_OUTPUT_SIZE,
    )
    .unwrap_or(standard)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zcash_primitives::transaction::fees::FeeRule as _;
    use zcash_primitives::transaction::fees::transparent::InputSize;
    use zcash_primitives::transaction::fees::zip317::P2PKH_STANDARD_OUTPUT_SIZE;

    fn report(fee_zat: u64, estimator_version: u32) -> Option<StandardFeeReport> {
        Some(StandardFeeReport {
            fee_zat,
            estimator_version,
        })
    }

    /// `FeeRule` carries no `PartialEq`, so equality reads field by field.
    fn assert_is_standard(rule: &zip317::FeeRule) {
        assert_eq!(rule.marginal_fee(), zip317::MARGINAL_FEE);
        assert_eq!(rule.grace_actions(), zip317::GRACE_ACTIONS);
        assert_eq!(
            rule.p2pkh_standard_input_size(),
            zip317::P2PKH_STANDARD_INPUT_SIZE
        );
        assert_eq!(
            rule.p2pkh_standard_output_size(),
            zip317::P2PKH_STANDARD_OUTPUT_SIZE
        );
    }

    /// The conventional fee of one P2PKH input and two P2PKH outputs under
    /// `rule`, a shape whose fee moves with the marginal fee.
    fn fee_for(rule: &zip317::FeeRule) -> u64 {
        rule.fee_required(
            &zcash_protocol::consensus::MAIN_NETWORK,
            zcash_protocol::consensus::BlockHeight::from_u32(1),
            [InputSize::STANDARD_P2PKH],
            [P2PKH_STANDARD_OUTPUT_SIZE, P2PKH_STANDARD_OUTPUT_SIZE],
            0,
            0,
            0,
            0,
        )
        .expect("a standard-shape fee cannot overflow")
        .into_u64()
    }

    #[test]
    fn a_node_fee_above_the_standard_raises_the_rule() {
        let rule = effective_fee_rule(report(7_000, STATIC_ZIP317_ESTIMATOR));

        let standard = fee_for(&zip317::FeeRule::standard());
        let raised = fee_for(&rule);
        assert!(
            raised > standard,
            "a 7_000 marginal fee must price above the standard rule's {standard}"
        );
        // One extra 2_000 per action across the shape's two padded actions.
        assert_eq!(raised - standard, 2 * (7_000 - 5_000));
        assert_eq!(rule.marginal_fee(), Zatoshis::const_from_u64(7_000));
    }

    #[test]
    fn a_node_fee_at_or_below_the_standard_keeps_the_standard_rule() {
        assert_is_standard(&effective_fee_rule(report(5_000, STATIC_ZIP317_ESTIMATOR)));
        assert_is_standard(&effective_fee_rule(report(1_000, STATIC_ZIP317_ESTIMATOR)));
    }

    #[test]
    fn a_node_fee_past_the_weight_ratio_cap_is_honored_at_the_cap() {
        let rule = effective_fee_rule(report(100_000, STATIC_ZIP317_ESTIMATOR));

        assert_eq!(
            rule.marginal_fee(),
            Zatoshis::const_from_u64(WEIGHT_RATIO_CAP * 5_000),
            "a report past the cap buys nothing past it, so the wallet pays the cap"
        );
    }

    #[test]
    fn no_report_keeps_the_standard_rule() {
        assert_is_standard(&effective_fee_rule(None));
    }

    #[test]
    fn an_unknown_estimator_version_is_ignored_wholesale() {
        assert_is_standard(&effective_fee_rule(report(7_000, 1)));
    }

    #[test]
    fn a_nonsense_fee_report_is_honored_at_the_cap() {
        let rule = effective_fee_rule(report(u64::MAX, STATIC_ZIP317_ESTIMATOR));

        assert_eq!(
            rule.marginal_fee(),
            Zatoshis::const_from_u64(WEIGHT_RATIO_CAP * 5_000)
        );
    }
}

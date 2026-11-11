//! The wallet's effective fee rule: ZIP 317, raised to whatever the
//! connected indexer's backend reports.

use zcash_primitives::transaction::fees::zip317;
use zcash_protocol::value::Zatoshis;

use zingo_netutils::lightwallet_protocol::StandardFee;

/// The estimator version whose reported marginal fee is the static ZIP 317
/// one, the only semantics the wallet can interpret. Any other version's
/// reports are ignored wholesale.
const STATIC_ZIP317_ESTIMATOR: u32 = 0;

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
/// prices with: the standard ZIP 317 rule whose marginal fee is the MAXIMUM
/// of the node's reported standard fee and the ZIP 317 standard marginal
/// fee. The wallet never pays below the standard, whatever the node
/// reports, and reports it cannot interpret (an unknown estimator version)
/// or never received (an old backend answers `Unimplemented`, any transport
/// failure) leave the standard rule standing.
pub(crate) fn effective_fee_rule(reported: Option<StandardFeeReport>) -> zip317::FeeRule {
    let standard = zip317::FeeRule::standard();
    let Some(reported) =
        reported.filter(|report| report.estimator_version == STATIC_ZIP317_ESTIMATOR)
    else {
        return standard;
    };
    let marginal_fee = reported.fee_zat.max(zip317::MARGINAL_FEE.into_u64());
    if marginal_fee == zip317::MARGINAL_FEE.into_u64() {
        return standard;
    }
    // `non_standard` refuses only zero P2PKH sizes, and the standard sizes
    // are nonzero. A marginal fee beyond Zatoshis' range (a nonsense report)
    // falls back with it, so a broken report degrades to the standard rule
    // rather than a panic.
    zip317::FeeRule::non_standard(
        Zatoshis::from_u64(marginal_fee).unwrap_or(zip317::MARGINAL_FEE),
        zip317::GRACE_ACTIONS,
        zip317::P2PKH_STANDARD_INPUT_SIZE,
        zip317::P2PKH_STANDARD_OUTPUT_SIZE,
    )
    .unwrap_or_else(|| standard)
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
    fn no_report_keeps_the_standard_rule() {
        assert_is_standard(&effective_fee_rule(None));
    }

    #[test]
    fn an_unknown_estimator_version_is_ignored_wholesale() {
        assert_is_standard(&effective_fee_rule(report(7_000, 1)));
    }

    #[test]
    fn a_nonsense_fee_report_degrades_to_the_standard_rule() {
        assert_is_standard(&effective_fee_rule(report(
            u64::MAX,
            STATIC_ZIP317_ESTIMATOR,
        )));
    }
}

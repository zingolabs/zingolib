//! The wallet's effective fee rule: ZIP 317 with the parameters of the
//! revision in force at the expected mining height, its marginal fee raised
//! to whatever the connected indexer's backend reports, capped at the
//! revision's weight ratio past which extra fee buys no inclusion.

use crate::config::ChainType;
use zcash_primitives::transaction::fees::zip317;
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};
use zcash_protocol::value::Zatoshis;

use zingo_netutils::lightwallet_protocol::StandardFee;

/// The estimator version whose reported marginal fee is the static ZIP 317
/// one, the only semantics the wallet can interpret. Any other version's
/// reports are ignored wholesale; a dynamic fee scheme announces itself as
/// a new version rather than repricing under the old semantics.
const STATIC_ZIP317_ESTIMATOR: u32 = 0;

/// The Mainnet height ZIP 2009 reduces ZIP 317 at, expected around
/// 19 November 2026. Off Mainnet the reduction rides NU7, so only Mainnet
/// carries the explicit number.
const MAINNET_FEE_REDUCTION_HEIGHT: u32 = 3_590_000;

/// The ZIP 317 weight-ratio cap, Revision 1: the block-template lottery
/// reads any fee above this multiple of the conventional fee as exactly
/// this multiple, so a marginal fee beyond it buys no inclusion probability
/// (<https://zips.z.cash/zip-0317#recommended-algorithm-for-block-template-construction>).
/// A report past the cap is honored at the cap: a lie costs at most the
/// cap's overcharge on one transaction, and a genuine demand above the cap
/// is indistinguishable on the network from the cap itself.
const REVISION_1_WEIGHT_RATIO_CAP: u64 = 4;

/// Revision 2 raises the cap to 10, so the fee cut does not narrow the
/// range of inclusion-influencing fees as sharply (ZIP 2009).
const REVISION_2_WEIGHT_RATIO_CAP: u64 = 10;

/// Revision 2's marginal fee, 1,000 zatoshi per logical action, in force
/// from the reduction height (ZIP 2009).
const REVISION_2_MARGINAL_FEE: u64 = 1_000;

/// The height Revision 2 reduces from on `chain`: Mainnet's explicit ZIP
/// 2009 number, NU7's activation height on Testnet, the chain's configured
/// NU7 on regtest.
fn fee_reduction_height(chain: &ChainType) -> Option<BlockHeight> {
    match chain {
        ChainType::Mainnet => Some(BlockHeight::from_u32(MAINNET_FEE_REDUCTION_HEIGHT)),
        chain => chain.activation_height(NetworkUpgrade::Nu7),
    }
}

/// The marginal fee and weight-ratio cap in force for a transaction
/// expected to mine at `expected_height`. An unknown height keeps
/// Revision 1: a wallet that overpays still mines, one that underpays may
/// not relay.
fn revision_parameters(chain: &ChainType, expected_height: Option<BlockHeight>) -> (u64, u64) {
    let reduced = fee_reduction_height(chain)
        .is_some_and(|height| expected_height.is_some_and(|expected| expected >= height));
    if reduced {
        (REVISION_2_MARGINAL_FEE, REVISION_2_WEIGHT_RATIO_CAP)
    } else {
        (zip317::MARGINAL_FEE.into_u64(), REVISION_1_WEIGHT_RATIO_CAP)
    }
}

/// The fee rule every proposal, shield, OP_RETURN send, and migration
/// prices with: the ZIP 317 rule of the revision in force at the expected
/// mining height, with the marginal fee raised to the node's reported
/// standard fee clamped between that revision's standard marginal fee and
/// its weight-ratio cap over it. The wallet never pays below the standard,
/// and never pays past the point where extra fee stops buying inclusion.
/// Reports it cannot interpret (an unknown estimator version) or never
/// received (an old backend answers `Unimplemented`, any transport failure)
/// leave the revision's standard rule standing.
pub(crate) fn effective_fee_rule(
    reported: Option<StandardFeeReport>,
    chain: &ChainType,
    expected_height: Option<BlockHeight>,
) -> zip317::FeeRule {
    let (standard_marginal, weight_ratio_cap) = revision_parameters(chain, expected_height);
    let marginal_fee = reported
        .filter(|report| report.estimator_version == STATIC_ZIP317_ESTIMATOR)
        .map(|report| {
            report
                .fee_zat
                .clamp(standard_marginal, weight_ratio_cap * standard_marginal)
        })
        .unwrap_or(standard_marginal);
    // The clamp bounds the fee at the cap, well inside Zatoshis' range, and
    // `non_standard` refuses only zero P2PKH sizes, which the standard sizes
    // are not. The revision's standard rule is the safe answer in both
    // impossible branches.
    zip317::FeeRule::non_standard(
        Zatoshis::from_u64(marginal_fee).unwrap_or(zip317::MARGINAL_FEE),
        zip317::GRACE_ACTIONS,
        zip317::P2PKH_STANDARD_INPUT_SIZE,
        zip317::P2PKH_STANDARD_OUTPUT_SIZE,
    )
    .unwrap_or_else(zip317::FeeRule::standard)
}

/// A backend's `GetStandardFee` report, cached in wallet state at first
/// contact. Copyable and proto-free, so an event can carry it unchanged
/// (zingolabs/zingolib#2820).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandardFeeReport {
    /// The reported marginal fee, zatoshis per logical action.
    pub fee_zat: u64,
    /// The backend fee-estimator version, `0` for the static ZIP 317
    /// marginal fee.
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

    fn nu7_height(chain: &ChainType) -> BlockHeight {
        chain
            .activation_height(NetworkUpgrade::Nu7)
            .expect("the schedule under test names NU7")
    }

    #[test]
    fn a_node_fee_above_the_standard_raises_the_rule() {
        let mainnet = ChainType::Mainnet;
        let before_the_reduction = mainnet
            .activation_height(NetworkUpgrade::Nu7)
            .expect("mainnet names NU7");
        let rule = effective_fee_rule(
            report(7_000, STATIC_ZIP317_ESTIMATOR),
            &mainnet,
            Some(before_the_reduction),
        );

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
        let mainnet = ChainType::Mainnet;
        let genesis_successor = Some(BlockHeight::from_u32(1));
        assert_is_standard(&effective_fee_rule(
            report(5_000, STATIC_ZIP317_ESTIMATOR),
            &mainnet,
            genesis_successor,
        ));
        assert_is_standard(&effective_fee_rule(
            report(1_000, STATIC_ZIP317_ESTIMATOR),
            &mainnet,
            genesis_successor,
        ));
    }

    #[test]
    fn a_node_fee_past_the_weight_ratio_cap_is_honored_at_the_cap() {
        let mainnet = ChainType::Mainnet;
        let rule = effective_fee_rule(
            report(100_000, STATIC_ZIP317_ESTIMATOR),
            &mainnet,
            Some(BlockHeight::from_u32(1)),
        );

        assert_eq!(
            rule.marginal_fee(),
            Zatoshis::const_from_u64(REVISION_1_WEIGHT_RATIO_CAP * 5_000),
            "a report past the cap buys nothing past it, so the wallet pays the cap"
        );
    }

    #[test]
    fn no_report_keeps_the_standard_rule() {
        let mainnet = ChainType::Mainnet;
        assert_is_standard(&effective_fee_rule(
            None,
            &mainnet,
            Some(BlockHeight::from_u32(1)),
        ));
    }

    #[test]
    fn an_unknown_estimator_version_is_ignored_wholesale() {
        let mainnet = ChainType::Mainnet;
        assert_is_standard(&effective_fee_rule(
            report(7_000, 1),
            &mainnet,
            Some(BlockHeight::from_u32(1)),
        ));
    }

    #[test]
    fn a_nonsense_fee_report_is_honored_at_the_cap() {
        let mainnet = ChainType::Mainnet;
        let rule = effective_fee_rule(
            report(u64::MAX, STATIC_ZIP317_ESTIMATOR),
            &mainnet,
            Some(BlockHeight::from_u32(1)),
        );

        assert_eq!(
            rule.marginal_fee(),
            Zatoshis::const_from_u64(REVISION_1_WEIGHT_RATIO_CAP * 5_000)
        );
    }

    #[test]
    fn mainnet_prices_revision_2_from_the_reduction_height_itself() {
        let mainnet = ChainType::Mainnet;
        let reduced = BlockHeight::from_u32(MAINNET_FEE_REDUCTION_HEIGHT);
        let rule = effective_fee_rule(
            report(1_000, STATIC_ZIP317_ESTIMATOR),
            &mainnet,
            Some(reduced),
        );
        assert_eq!(rule.marginal_fee(), Zatoshis::const_from_u64(1_000));

        let day_before = reduced - 1;
        let rule = effective_fee_rule(
            report(1_000, STATIC_ZIP317_ESTIMATOR),
            &mainnet,
            Some(day_before),
        );
        assert_eq!(
            rule.marginal_fee(),
            zip317::MARGINAL_FEE,
            "the day before the reduction, the 1_000 report clamps up to the standard"
        );
    }

    #[test]
    fn the_revision_2_cap_is_ten_conventional_fees() {
        let mainnet = ChainType::Mainnet;
        let reduced = BlockHeight::from_u32(MAINNET_FEE_REDUCTION_HEIGHT);
        let rule = effective_fee_rule(
            report(100_000, STATIC_ZIP317_ESTIMATOR),
            &mainnet,
            Some(reduced),
        );

        assert_eq!(
            rule.marginal_fee(),
            Zatoshis::const_from_u64(REVISION_2_WEIGHT_RATIO_CAP * REVISION_2_MARGINAL_FEE)
        );
    }

    #[test]
    fn no_report_after_the_reduction_keeps_the_revision_2_standard_rule() {
        let mainnet = ChainType::Mainnet;
        let reduced = BlockHeight::from_u32(MAINNET_FEE_REDUCTION_HEIGHT);
        let rule = effective_fee_rule(None, &mainnet, Some(reduced));

        assert_eq!(rule.marginal_fee(), Zatoshis::const_from_u64(1_000));
        assert_eq!(rule.grace_actions(), zip317::GRACE_ACTIONS);
        assert_eq!(
            rule.p2pkh_standard_output_size(),
            zip317::P2PKH_STANDARD_OUTPUT_SIZE
        );
    }

    #[test]
    fn testnet_reduces_with_nu7() {
        let testnet = ChainType::Testnet;
        let nu7 = nu7_height(&testnet);
        let rule = effective_fee_rule(report(1_000, STATIC_ZIP317_ESTIMATOR), &testnet, Some(nu7));
        assert_eq!(rule.marginal_fee(), Zatoshis::const_from_u64(1_000));

        let rule = effective_fee_rule(
            report(1_000, STATIC_ZIP317_ESTIMATOR),
            &testnet,
            Some(nu7 - 1),
        );
        assert_eq!(rule.marginal_fee(), zip317::MARGINAL_FEE);
    }

    #[test]
    fn a_regtest_chain_reduces_with_its_configured_nu7() {
        let reduced = ChainType::Regtest(crate::testutils::mock_activation_heights());
        let rule = effective_fee_rule(
            report(1_000, STATIC_ZIP317_ESTIMATOR),
            &reduced,
            Some(BlockHeight::from_u32(1)),
        );
        assert_eq!(rule.marginal_fee(), Zatoshis::const_from_u64(1_000));

        let unreduced = ChainType::Regtest(crate::testutils::mock_activation_heights_with(|era| {
            era.set_nu7(None)
        }));
        let rule = effective_fee_rule(
            report(1_000, STATIC_ZIP317_ESTIMATOR),
            &unreduced,
            Some(BlockHeight::from_u32(100)),
        );
        assert_eq!(
            rule.marginal_fee(),
            zip317::MARGINAL_FEE,
            "a regtest chain that never activates NU7 never reduces"
        );
    }

    #[test]
    fn an_unknown_height_keeps_the_revision_1_parameters() {
        let mainnet = ChainType::Mainnet;
        let rule = effective_fee_rule(report(1_000, STATIC_ZIP317_ESTIMATOR), &mainnet, None);
        assert_eq!(
            rule.marginal_fee(),
            zip317::MARGINAL_FEE,
            "a wallet that cannot name the mining height overpays rather than underpays"
        );
    }
}

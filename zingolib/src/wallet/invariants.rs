//! Invariant checks over the wallet's whole record set.

use std::collections::BTreeSet;
use std::fmt;

use pepper_sync::wallet::{
    IronwoodNote, KeyIdInterface, NoteInterface, OrchardNote, OutputId, OutputInterface,
    SaplingNote, TransparentCoin,
};
use zcash_primitives::transaction::TxId;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::PoolType;
use zcash_protocol::value::Zatoshis;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::LightWallet;
use super::balance::AccountBalance;
use super::error::BalanceError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceLine {
    Confirmed,
    Unconfirmed,
}

impl BalanceLine {
    fn of(status: ConfirmationStatus) -> Option<Self> {
        match status {
            ConfirmationStatus::Confirmed(_) => Some(Self::Confirmed),
            ConfirmationStatus::Calculated(_)
            | ConfirmationStatus::Transmitted(_)
            | ConfirmationStatus::Mempool(_) => Some(Self::Unconfirmed),
            ConfirmationStatus::Failed(_) => None,
        }
    }
}

impl fmt::Display for BalanceLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Confirmed => write!(f, "confirmed"),
            Self::Unconfirmed => write!(f, "unconfirmed"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvariantViolation {
    #[error("{pool:?} output {output_id:?} is stored in the record of {record}")]
    ForeignRecord {
        pool: PoolType,
        output_id: OutputId,
        record: TxId,
    },
    #[error("{pool:?} output {output_id:?} appears in two records")]
    DuplicateOutput { pool: PoolType, output_id: OutputId },
    #[error(
        "{pool:?} output {output_id:?} names spending transaction {spender}, which the wallet does not hold"
    )]
    SpenderUnheld {
        pool: PoolType,
        output_id: OutputId,
        spender: TxId,
    },
    #[error("{pool:?} output {output_id:?} is marked spent by failed transaction {spender}")]
    SpenderFailed {
        pool: PoolType,
        output_id: OutputId,
        spender: TxId,
    },
    #[error(
        "{pool:?} output {output_id:?} is marked spent by confirmed transaction {spender}, which carries none of its spend links"
    )]
    SpendLinkAbsent {
        pool: PoolType,
        output_id: OutputId,
        spender: TxId,
    },
    #[error(
        "{pool:?} output {output_id:?} is marked spent by pending transaction {spender}, which the derivation does not report"
    )]
    PendingSpendUnderived {
        pool: PoolType,
        output_id: OutputId,
        spender: TxId,
    },
    #[error(
        "confirmed transaction {spender} spends {pool:?} output {output_id:?} with no recorded spend"
    )]
    ConfirmedSpendUnrecorded {
        pool: PoolType,
        output_id: OutputId,
        spender: TxId,
    },
    #[error("the account balance is unavailable: {0}")]
    BalanceUnavailable(#[source] BalanceError),
    #[error("{pool:?} {line} balance reports {reported:?}, the raw notes sum to {derived}")]
    BalanceMismatch {
        pool: PoolType,
        line: BalanceLine,
        reported: Zatoshis,
        derived: u64,
    },
}

#[must_use]
pub fn violation_report(violations: &[InvariantViolation]) -> String {
    violations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

impl LightWallet {
    #[must_use]
    pub fn invariant_violations(&self) -> Vec<InvariantViolation> {
        let spends = self.spend_view();
        [
            output_violations::<TransparentCoin>(self, &spends),
            output_violations::<SaplingNote>(self, &spends),
            output_violations::<OrchardNote>(self, &spends),
            output_violations::<IronwoodNote>(self, &spends),
            unrecorded_confirmed_spends::<SaplingNote>(self),
            unrecorded_confirmed_spends::<OrchardNote>(self),
            unrecorded_confirmed_spends::<IronwoodNote>(self),
            unrecorded_confirmed_spends::<TransparentCoin>(self),
            balance_violations::<SaplingNote>(self, &spends),
            balance_violations::<OrchardNote>(self, &spends),
            balance_violations::<IronwoodNote>(self, &spends),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// - Panics when the wallet violates an invariant.
pub fn assert_wallet_invariants(wallet: &LightWallet) {
    let violations = wallet.invariant_violations();
    assert!(
        violations.is_empty(),
        "wallet invariants violated:\n{}",
        violation_report(&violations)
    );
}

fn output_violations<Op: OutputInterface>(
    wallet: &LightWallet,
    spends: &pepper_sync::SpendView<'_>,
) -> Vec<InvariantViolation> {
    let mut seen = BTreeSet::new();
    wallet
        .wallet_transactions
        .values()
        .flat_map(|transaction| {
            Op::transaction_outputs(transaction)
                .iter()
                .map(move |output| (transaction.txid(), output))
        })
        .flat_map(|(record, output)| {
            let output_id = output.output_id();
            [
                (output_id.txid() != record).then_some(InvariantViolation::ForeignRecord {
                    pool: Op::POOL_TYPE,
                    output_id,
                    record,
                }),
                (!seen.insert(output_id)).then_some(InvariantViolation::DuplicateOutput {
                    pool: Op::POOL_TYPE,
                    output_id,
                }),
                spend_mark_violation(wallet, spends, output),
            ]
        })
        .flatten()
        .collect()
}

fn spend_mark_violation<Op: OutputInterface>(
    wallet: &LightWallet,
    spends: &pepper_sync::SpendView<'_>,
    output: &Op,
) -> Option<InvariantViolation> {
    let pool = Op::POOL_TYPE;
    let output_id = output.output_id();
    let spender = output.spending_transaction()?;
    let Some(spending_record) = wallet.wallet_transactions.get(&spender) else {
        return Some(InvariantViolation::SpenderUnheld {
            pool,
            output_id,
            spender,
        });
    };
    match spending_record.status() {
        ConfirmationStatus::Failed(_) => Some(InvariantViolation::SpenderFailed {
            pool,
            output_id,
            spender,
        }),
        ConfirmationStatus::Confirmed(_) => output
            .spend_link()
            .is_some_and(|link| !Op::transaction_inputs(spending_record).contains(&&link))
            .then_some(InvariantViolation::SpendLinkAbsent {
                pool,
                output_id,
                spender,
            }),
        ConfirmationStatus::Calculated(_)
        | ConfirmationStatus::Transmitted(_)
        | ConfirmationStatus::Mempool(_) => (!spends.derives_pending_spend(output_id, spender))
            .then_some(InvariantViolation::PendingSpendUnderived {
                pool,
                output_id,
                spender,
            }),
    }
}

fn unrecorded_confirmed_spends<Op: OutputInterface>(
    wallet: &LightWallet,
) -> Vec<InvariantViolation> {
    wallet
        .wallet_transactions
        .values()
        .filter(|transaction| transaction.status().is_confirmed())
        .map(|spending_record| {
            (
                spending_record.txid(),
                Op::transaction_inputs(spending_record),
            )
        })
        .filter(|(_, inputs)| !inputs.is_empty())
        .flat_map(|(spender, inputs)| {
            wallet
                .wallet_transactions
                .values()
                .flat_map(Op::transaction_outputs)
                .filter(move |output| {
                    output.spending_transaction().is_none()
                        && output
                            .spend_link()
                            .is_some_and(|link| inputs.contains(&&link))
                })
                .map(move |output| InvariantViolation::ConfirmedSpendUnrecorded {
                    pool: Op::POOL_TYPE,
                    output_id: output.output_id(),
                    spender,
                })
        })
        .collect()
}

fn balance_violations<Op: NoteInterface>(
    wallet: &LightWallet,
    spends: &pepper_sync::SpendView<'_>,
) -> Vec<InvariantViolation> {
    let Some(account) = wallet.unified_key_store.keys().copied().next() else {
        return Vec::new();
    };
    let balance = match wallet.account_balance(account) {
        Ok(balance) => balance,
        Err(error) => return vec![InvariantViolation::BalanceUnavailable(error)],
    };
    let derived_sum = |line: BalanceLine| -> u64 {
        wallet
            .wallet_transactions
            .values()
            .filter(|transaction| BalanceLine::of(transaction.status()) == Some(line))
            .flat_map(Op::transaction_outputs)
            .filter(|&output| {
                output.key_id().account_id() == account
                    && output.value() > MARGINAL_FEE.into_u64()
                    && spends.state(output).is_unspent()
            })
            .map(OutputInterface::value)
            .sum()
    };
    let (reported_confirmed, reported_unconfirmed) = balance_lines::<Op>(&balance);
    [
        (BalanceLine::Confirmed, reported_confirmed),
        (BalanceLine::Unconfirmed, reported_unconfirmed),
    ]
    .into_iter()
    .filter_map(|(line, reported)| {
        let reported = reported?;
        let derived = derived_sum(line);
        (reported.into_u64() != derived).then_some(InvariantViolation::BalanceMismatch {
            pool: Op::POOL_TYPE,
            line,
            reported,
            derived,
        })
    })
    .collect()
}

fn balance_lines<Op: OutputInterface>(
    balance: &AccountBalance,
) -> (Option<Zatoshis>, Option<Zatoshis>) {
    match Op::POOL_TYPE {
        zcash_protocol::PoolType::SAPLING => (
            balance.confirmed_sapling_balance,
            balance.unconfirmed_sapling_balance,
        ),
        zcash_protocol::PoolType::ORCHARD => (
            balance.confirmed_orchard_balance,
            balance.unconfirmed_orchard_balance,
        ),
        zcash_protocol::PoolType::IRONWOOD => (
            balance.confirmed_ironwood_balance,
            balance.unconfirmed_ironwood_balance,
        ),
        zcash_protocol::PoolType::Transparent => (
            balance.confirmed_transparent_balance,
            balance.unconfirmed_transparent_balance,
        ),
    }
}

#[cfg(test)]
mod tests {
    use zcash_primitives::transaction::TxId;
    use zingo_status::confirmation_status::ConfirmationStatus;

    use pepper_sync::wallet::OutputInterface as _;

    use super::InvariantViolation;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use zingo_test_vectors::seeds;

    const NOTE_VALUE: u64 = 100_000;
    const UNHELD_SPENDER_TXID_BYTES: [u8; 32] = [9; 32];
    const FAILED_SPENDER_TXID_BYTES: [u8; 32] = [7; 32];
    const FAILED_SPENDER_HEIGHT: u32 = 5;

    fn funded_wallet() -> LightWallet {
        SyntheticWalletBuilder::new(seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(NOTE_VALUE)
            .build()
    }

    fn mark_orchard_notes_spent_by(wallet: &mut LightWallet, spender: TxId) {
        for transaction in wallet.wallet_transactions.values_mut() {
            for note in transaction.orchard_notes_mut() {
                note.set_spending_transaction(Some(spender));
            }
        }
    }

    #[test]
    fn a_healthy_wallet_has_no_violations() {
        let violations = funded_wallet().invariant_violations();
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn a_spend_mark_naming_an_unheld_transaction_is_a_spender_unheld_violation() {
        let unheld = TxId::from_bytes(UNHELD_SPENDER_TXID_BYTES);
        let mut wallet = funded_wallet();
        mark_orchard_notes_spent_by(&mut wallet, unheld);

        let violations = wallet.invariant_violations();
        assert!(
            matches!(
                violations.as_slice(),
                [InvariantViolation::SpenderUnheld { spender, .. }] if *spender == unheld
            ),
            "{violations:?}"
        );
    }

    #[test]
    fn a_spend_mark_held_by_a_failed_transaction_is_a_spender_failed_violation() {
        let failed = TxId::from_bytes(FAILED_SPENDER_TXID_BYTES);
        let mut wallet = funded_wallet();
        mark_orchard_notes_spent_by(&mut wallet, failed);
        wallet.wallet_transactions.insert(
            failed,
            pepper_sync::wallet::WalletTransaction::new_for_test(
                failed,
                ConfirmationStatus::Failed(FAILED_SPENDER_HEIGHT.into()),
            ),
        );

        let violations = wallet.invariant_violations();
        assert!(
            matches!(
                violations.as_slice(),
                [InvariantViolation::SpenderFailed { spender, .. }] if *spender == failed
            ),
            "{violations:?}"
        );
    }
}

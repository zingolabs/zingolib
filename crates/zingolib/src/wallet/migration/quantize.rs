//! Canonical quantization: decomposing a balance into ZIP 318's standard
//! `{1, 2, 5} x 10^k` denominations
//! (<https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>).

use zcash_protocol::value::Zatoshis;

use super::params::MigrationParams;

/// The result of decomposing a value into canonical denominations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denominations {
    /// One entry per Ironwood output to create. Every value is a member of
    /// [`MigrationParams::denominations`]. Ordered largest first.
    outputs: Vec<Zatoshis>,
    /// The leftover below the smallest denomination that has no canonical denomination.
    /// Always strictly less than [`MigrationParams::max_residual_value`]. The
    /// migration folds this into the fee instead of creating a non-standard
    /// note.
    remainder: Zatoshis,
}

impl Denominations {
    /// The output notes to create, each a canonical denomination, largest first.
    pub fn outputs(&self) -> &[Zatoshis] {
        &self.outputs
    }

    /// The sub-denomination leftover to fold into the fee.
    pub fn remainder(&self) -> Zatoshis {
        self.remainder
    }

    /// Total value across all canonical outputs.
    pub fn total(&self) -> Zatoshis {
        // Bounded by the input value, itself a valid `Zatoshis`, so in range.
        Zatoshis::const_from_u64(self.outputs.iter().map(|z| u64::from(*z)).sum())
    }
}

/// Decompose `value` into the canonical `{1, 2, 5} x 10^k` denominations.
///
/// Greedy, largest denomination first. Because each denomination is exactly
/// ten times the next, greedy decomposition also minimizes the number of
/// outputs. The returned [`Denominations::outputs`] sum to `value` minus
/// [`Denominations::remainder`], and the remainder is always below the dust
/// floor.
///
/// Pass the amount that will actually land in Ironwood, the spendable Orchard
/// total minus the fee. The caller then folds the remainder into the fee, so
/// the wallet empties the Orchard pool.
pub fn decompose(value: Zatoshis, params: &MigrationParams) -> Denominations {
    let mut remaining = u64::from(value);
    let mut outputs = Vec::new();

    for &denomination in &params.denominations {
        let count = remaining / denomination;
        remaining -= count * denomination;
        for _ in 0..count {
            outputs.push(Zatoshis::const_from_u64(denomination));
        }
    }

    Denominations {
        outputs,
        // `remaining` is what is left after removing every smallest-denomination unit,
        // so it is strictly below the smallest denomination and trivially in range.
        remainder: Zatoshis::const_from_u64(remaining),
    }
}

#[cfg(test)]
mod tests;

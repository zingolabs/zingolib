# Claim: AllFunds selection — TDD fix (grilling session, 2026-07-17)

The #2419 review verified that `select_spendable_notes` panics on
`TargetValue::AllFunds` (zingolib/src/wallet/zcb_traits.rs), the sole
verified finding without an owner. The user ratified a TDD pair on
branch `fix/allfunds-pure-selection` (based on feat/ironwood,
independent of PR #2479): first a red test pinning the contract, then
the fix implemented as a side-effect-free pure function
(functional-core/imperative-shell — the wallet reads gather
candidates at the edge; a pure `all_funds_selection` decides).

Contract: `MaxSpendable` selects every spend-worthy note (dust
discipline `> MARGINAL_FEE`, matching budgeted selection and the
drain); `Everything` refuses with a typed error until its
unspendable-funds audit exists, because a wrong Ok would silently
strand funds and the panic it replaces killed the process.

## File claims

- `zingolib/src/wallet/zcb_traits.rs` (AllFunds arm, pure functions,
  test module; NOTE: PR #2478 also touches this file in the OutputRef
  label region — disjoint hunks expected)
- `zingolib/src/wallet/error.rs` (one new WalletError variant)

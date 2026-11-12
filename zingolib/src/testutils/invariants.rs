use std::collections::BTreeSet;

use pepper_sync::wallet::{
    IronwoodNote, KeyIdInterface, NoteInterface, OrchardNote, OutputId, SaplingNote,
};
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::PoolType;

use crate::testutils::mock_indexer::PoolNullifier;
use crate::wallet::LightWallet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "{pool:?} note {output_id:?} is counted spendable while its nullifier is revealed on chain"
)]
pub struct RevealedSpendableNote {
    pub pool: PoolType,
    pub output_id: OutputId,
}

#[must_use]
pub fn wallet_reconciliation_violations(
    wallet: &LightWallet,
    revealed: &BTreeSet<PoolNullifier>,
) -> Vec<RevealedSpendableNote> {
    let Some(account) = wallet.unified_key_store.keys().copied().next() else {
        return Vec::new();
    };
    let spends = wallet.spend_view();
    [
        revealed_spendable_notes::<SaplingNote>(
            wallet,
            &spends,
            account,
            revealed,
            PoolNullifier::Sapling,
        ),
        revealed_spendable_notes::<OrchardNote>(
            wallet,
            &spends,
            account,
            revealed,
            PoolNullifier::Orchard,
        ),
        revealed_spendable_notes::<IronwoodNote>(
            wallet,
            &spends,
            account,
            revealed,
            PoolNullifier::Ironwood,
        ),
    ]
    .concat()
}

fn revealed_spendable_notes<N: NoteInterface>(
    wallet: &LightWallet,
    spends: &pepper_sync::SpendView<'_>,
    account: zip32::AccountId,
    revealed: &BTreeSet<PoolNullifier>,
    pool_nullifier: impl Fn(N::Nullifier) -> PoolNullifier,
) -> Vec<RevealedSpendableNote> {
    wallet
        .wallet_transactions
        .values()
        .filter(|transaction| !transaction.status().is_failed())
        .flat_map(N::transaction_outputs)
        .filter(|&note| {
            spends.state(note).is_unspent()
                && note.key_id().account_id() == account
                && note.value() > MARGINAL_FEE.into_u64()
                && note
                    .nullifier()
                    .is_some_and(|nullifier| revealed.contains(&pool_nullifier(nullifier)))
        })
        .map(|note| RevealedSpendableNote {
            pool: N::POOL_TYPE,
            output_id: note.output_id(),
        })
        .collect()
}

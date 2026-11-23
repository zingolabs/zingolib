use zcash_protocol::{ShieldedPool, TxId};
use zl_common::chain::CompactTx;

/// Returns the transaction Id
pub(crate) fn get_compact_txid(compact_tx: &CompactTx) -> TxId {
    compact_tx.txid
}

pub(crate) fn shielded_output_count(compact_tx: &CompactTx, pool: ShieldedPool) -> u32 {
    match pool {
        ShieldedPool::Sapling => compact_tx.outputs.len(),
        ShieldedPool::Orchard => compact_tx.actions.len(),
        ShieldedPool::Ironwood => compact_tx.ironwood_actions.len(),
    }
    .try_into()
    .expect("note commitment tree sizes are u32; a transaction cannot carry more outputs")
}

pub(crate) fn shielded_input_count(compact_tx: &CompactTx, pool: ShieldedPool) -> u32 {
    match pool {
        ShieldedPool::Sapling => compact_tx.spends.len(),
        ShieldedPool::Orchard => compact_tx.actions.len(),
        ShieldedPool::Ironwood => compact_tx.ironwood_actions.len(),
    }
    .try_into()
    .expect("note commitment tree sizes are u32; a transaction cannot carry more inputs")
}

#[cfg(test)]
pub(crate) const ALL_POOLS: [ShieldedPool; 3] = [
    ShieldedPool::Sapling,
    ShieldedPool::Orchard,
    ShieldedPool::Ironwood,
];

#[cfg(test)]
pub(crate) fn compact_tx(
    spends: usize,
    outputs: usize,
    actions: usize,
    ironwood_actions: usize,
) -> CompactTx {
    CompactTx {
        index: 0,
        txid: TxId::from_bytes([0; 32]),
        fee: 0,
        spends: vec![zl_common::chain::CompactSaplingSpend { nullifier: [0; 32] }; spends],
        outputs: vec![
            zl_common::chain::CompactSaplingOutput {
                cmu: vec![0; 32],
                ephemeral_key: vec![0; 32],
                ciphertext: vec![0; 52],
            };
            outputs
        ],
        actions: vec![
            zl_common::chain::CompactOrchardAction {
                nullifier: [0; 32],
                cmx: vec![0; 32],
                ephemeral_key: vec![0; 32],
                ciphertext: vec![0; 52],
            };
            actions
        ],
        ironwood_actions: vec![
            zl_common::chain::CompactOrchardAction {
                nullifier: [0; 32],
                cmx: vec![0; 32],
                ephemeral_key: vec![0; 32],
                ciphertext: vec![0; 52],
            };
            ironwood_actions
        ],
        vin: vec![],
        vout: vec![],
    }
}

/// An otherwise-empty compact block holding `vtx`, for tests exercising per-block aggregation.
#[cfg(test)]
pub(crate) fn empty_compact_block(vtx: Vec<CompactTx>) -> zl_common::chain::CompactBlock {
    zl_common::chain::CompactBlock {
        height: zcash_protocol::consensus::BlockHeight::from_u32(0),
        hash: zcash_primitives::block::BlockHash([0; 32]),
        prev_hash: zcash_primitives::block::BlockHash([0; 32]),
        time: 0,
        header: vec![],
        vtx,
        chain_metadata: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_count_reads_each_pools_own_field() {
        let transaction = compact_tx(1, 2, 3, 4);
        assert_eq!(
            shielded_output_count(&transaction, ShieldedPool::Sapling),
            2
        );
        assert_eq!(
            shielded_output_count(&transaction, ShieldedPool::Orchard),
            3
        );
        assert_eq!(
            shielded_output_count(&transaction, ShieldedPool::Ironwood),
            4
        );
    }

    #[test]
    fn input_count_reads_spends_for_sapling_and_actions_for_action_pools() {
        let transaction = compact_tx(1, 2, 3, 4);
        assert_eq!(shielded_input_count(&transaction, ShieldedPool::Sapling), 1);
        assert_eq!(shielded_input_count(&transaction, ShieldedPool::Orchard), 3);
        assert_eq!(
            shielded_input_count(&transaction, ShieldedPool::Ironwood),
            4
        );
    }

    #[test]
    fn empty_transaction_counts_zero_for_every_pool() {
        let transaction = compact_tx(0, 0, 0, 0);
        for pool in ALL_POOLS {
            assert_eq!(shielded_output_count(&transaction, pool), 0);
            assert_eq!(shielded_input_count(&transaction, pool), 0);
        }
    }
}

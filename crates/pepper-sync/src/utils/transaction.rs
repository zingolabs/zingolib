use zcash_protocol::{ShieldedPool, TxId};
use zl_netutils::lightwallet_protocol::CompactTx;

#[cfg(test)]
use zl_netutils::lightwallet_protocol::{
    CompactOrchardAction, CompactSaplingOutput, CompactSaplingSpend,
};

/// Returns the transaction Id
pub(crate) fn get_compact_txid(compact_tx: &CompactTx) -> TxId {
    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(&compact_tx.txid);
    TxId::from_bytes(txid_bytes)
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
        txid: vec![0; 32],
        fee: 0,
        spends: vec![CompactSaplingSpend::default(); spends],
        outputs: vec![CompactSaplingOutput::default(); outputs],
        actions: vec![CompactOrchardAction::default(); actions],
        ironwood_actions: vec![CompactOrchardAction::default(); ironwood_actions],
        vin: vec![],
        vout: vec![],
    }
}

#[cfg(test)]
mod tests;
